//! 取り込み・保存・開く・書き出し・絞り込み・並べ替えの速さを測る。
//!
//! ```text
//! cargo run --release -p yy-sheet --example bench -- 列数 行数 [作業フォルダ]
//! ```

use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::time::Instant;

use yy_sheet::{Context, Document, Workbook, csv, yys};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cols: usize = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(20);
    let rows: usize = args
        .get(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(1_000_000);
    let dir = PathBuf::from(args.get(3).map_or("bench-out", String::as_str));
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("in.csv");
    let t = Instant::now();
    {
        let mut w = BufWriter::with_capacity(1 << 20, std::fs::File::create(&src).unwrap());
        let head: Vec<String> = (0..cols).map(|c| format!("列{c}")).collect();
        writeln!(w, "{}", head.join(",")).unwrap();
        let mut x: u64 = 88172645463325252;
        let mut line = String::new();
        for r in 0..rows {
            line.clear();
            for c in 0..cols {
                if c > 0 {
                    line.push(',');
                }
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                match c % 4 {
                    0 => line.push_str(&(x % 1_000_000).to_string()),
                    1 => line.push_str(&format!("{}.{:02}", x % 10_000, x % 100)),
                    2 => {
                        line.push_str(["東京", "大阪", "名古屋", "福岡", "札幌"][(x % 5) as usize])
                    }
                    _ => line.push_str(&format!("ID{}", (x % 100_000) + r as u64 % 7)),
                }
            }
            line.push('\n');
            w.write_all(line.as_bytes()).unwrap();
        }
    }
    let size = std::fs::metadata(&src).unwrap().len();
    println!(
        "CSV {} 列 × {} 行 = {:.1} MB（作成 {:.1} 秒）",
        cols,
        rows,
        size as f64 / 1e6,
        t.elapsed().as_secs_f64()
    );

    let ctx = Context::new(yy_sheet::budget::DEFAULT_LIMIT, dir.clone());
    let pv = csv::preview(&src).unwrap();
    let t = Instant::now();
    let sheet = csv::import(&ctx, &src, &pv.options, &|_, _| true).unwrap();
    let secs = t.elapsed().as_secs_f64();
    println!(
        "取り込み {:.2} 秒（{:.0} MB/s）{} 行 キャッシュ {:.0} MB",
        secs,
        size as f64 / 1e6 / secs,
        sheet.table.rows,
        ctx.budget.used(yy_sheet::budget::Part::Cache) as f64 / 1e6
    );

    let mut doc = Document::with_book(
        ctx.clone(),
        Workbook {
            sheets: vec![sheet],
            date_system: Default::default(),
        },
    );
    let out = dir.join("out.yys");
    let _ = std::fs::remove_file(&out);
    let t = Instant::now();
    yys::save(&mut doc, &out, &mut |_, _| true).unwrap();
    println!(
        "保存 {:.2} 秒 {:.1} MB",
        t.elapsed().as_secs_f64(),
        std::fs::metadata(&out).unwrap().len() as f64 / 1e6
    );
    let t = Instant::now();
    let d2 = yys::open(ctx.clone(), &out).unwrap();
    println!("開く {:.3} 秒", t.elapsed().as_secs_f64());
    let t = Instant::now();
    doc.edit(|b, ctx| b.sheets[0].set(ctx, 10, 0, 1.0.into()))
        .unwrap();
    yys::save(&mut doc, &out, &mut |_, _| true).unwrap();
    println!("1 セル直して保存 {:.3} 秒", t.elapsed().as_secs_f64());
    let t = Instant::now();
    let csv_out = dir.join("out.csv");
    csv::export(
        &ctx,
        &d2.book.sheets[0],
        Default::default(),
        &csv_out,
        &csv::ExportOptions::default(),
        None,
        &|_, _| true,
    )
    .unwrap();
    println!("書き出し {:.2} 秒", t.elapsed().as_secs_f64());

    use yy_sheet::query::{self, Cmp, ColFilter, Cond, SortKey, TextOp};
    let table = &d2.book.sheets[0].table;
    let t = Instant::now();
    let (_, counts) = query::filter(
        &ctx,
        table,
        &[
            ColFilter {
                col: 0,
                cond: Cond::Number {
                    op: Cmp::Lt,
                    value: 500_000.0,
                },
            },
            ColFilter {
                col: 2.min(cols as u32 - 1),
                cond: Cond::Text {
                    op: TextOp::Equals,
                    pattern: "東京".into(),
                    negate: false,
                },
            },
        ],
    )
    .unwrap();
    println!(
        "絞り込み（2 段階）{:.2} 秒 → {:?} 行",
        t.elapsed().as_secs_f64(),
        counts
    );
    let t = Instant::now();
    let order = query::sort(
        &ctx,
        table,
        &[SortKey {
            col: 0,
            desc: false,
        }],
        None,
    )
    .unwrap();
    println!("並べ替え（数値 1 キー）{:.2} 秒", t.elapsed().as_secs_f64());
    if cols >= 3 {
        let t = Instant::now();
        query::sort(
            &ctx,
            table,
            &[
                SortKey {
                    col: 2,
                    desc: false,
                },
                SortKey { col: 1, desc: true },
                SortKey {
                    col: 0,
                    desc: false,
                },
            ],
            None,
        )
        .unwrap();
        println!(
            "並べ替え（3 キー・文字列を含む）{:.2} 秒",
            t.elapsed().as_secs_f64()
        );
    }
    let t = Instant::now();
    query::permute(&ctx, table, &order).unwrap();
    println!("並べ替えの確定 {:.2} 秒", t.elapsed().as_secs_f64());

    if cols >= 3 {
        let mut d3 = d2;
        let last = cols as u32 + 1;
        for (i, f) in [
            "=SUMIFS(B:B,C:C,\"東京\",A:A,\">500000\")",
            "=COUNTIFS(C:C,\"大阪\")",
            "=XLOOKUP(999999,A:A,C:C,\"なし\")",
        ]
        .iter()
        .enumerate()
        {
            let t = Instant::now();
            d3.edit(|b, ctx| {
                b.sheets[0]
                    .set_formula(ctx, i as u64, last, f)
                    .map_err(std::io::Error::other)
            })
            .unwrap();
            println!(
                "{f} → {:?}（追加して再計算 {:.2} 秒）",
                d3.book.sheets[0].get(&ctx, i as u64, last).unwrap(),
                t.elapsed().as_secs_f64()
            );
        }
        // 集計表: 条件だけが違う SUMIFS を 1000 個まとめて入れる（貼り付けと同じ 1 回の編集）
        for (label, make) in [
            (
                "SUMIFS 1000 個",
                (|k: usize| {
                    let city = ["東京", "大阪", "名古屋", "福岡", "札幌"][k % 5];
                    format!("=SUMIFS(B:B,C:C,\"{city}\",D:D,\"ID{}\")", k * 37 % 100_000)
                }) as fn(usize) -> String,
            ),
            (
                "XLOOKUP 1000 個",
                (|k: usize| format!("=XLOOKUP(\"ID{}\",D:D,A:A,\"なし\")", k * 91 % 100_000))
                    as fn(usize) -> String,
            ),
        ] {
            let col = last + 1 + (label.len() % 2) as u32;
            let t = Instant::now();
            d3.edit(|b, ctx| {
                for k in 0..1000 {
                    b.sheets[0]
                        .set_formula(ctx, k as u64, col, &make(k))
                        .map_err(std::io::Error::other)?;
                }
                Ok(())
            })
            .unwrap();
            println!(
                "{label}（1 回の編集で入れて計算）{:.2} 秒 → 1 つ目 {:?}",
                t.elapsed().as_secs_f64(),
                d3.book.sheets[0].get(&ctx, 0, col).unwrap()
            );
        }
        // 共有式: 表の全行に式を下へコピーする（1 つの式で持ち、まとめて計算する）
        let rows = d3.book.sheets[0].table.rows;
        for (label, f, col) in [
            ("演算の共有式", "=A2*2+B2", last + 4),
            (
                "XLOOKUP の共有式（表どうしの結合）",
                "=XLOOKUP(D2,D:D,A:A)",
                last + 5,
            ),
        ] {
            let t = Instant::now();
            d3.edit(|b, ctx| {
                let s = &mut b.sheets[0];
                s.set_formula(ctx, 1, col, f)
                    .map_err(std::io::Error::other)?;
                s.fill_down(ctx, 1, rows, col, col)
                    .map_err(std::io::Error::other)
            })
            .unwrap();
            println!(
                "{label} {f} を {rows} 行 {:.2} 秒 → 最後 {:?}",
                t.elapsed().as_secs_f64(),
                d3.book.sheets[0].get(&ctx, rows, col).unwrap()
            );
        }
        // 元のデータの 1 セルを直す: 関わる式だけ（ここではすべて）を計算し直す
        let t = Instant::now();
        d3.edit(|b, ctx| b.sheets[0].set(ctx, 5, 1, 1.0.into()))
            .unwrap();
        println!(
            "データの 1 セルを直して再計算 {:.2} 秒",
            t.elapsed().as_secs_f64()
        );
        let t = Instant::now();
        d3.edit(|b, ctx| b.sheets[0].set(ctx, 3, last + 9, 1.0.into()))
            .unwrap();
        println!("関係のないセルを直す {:.4} 秒", t.elapsed().as_secs_f64());
    }
}
