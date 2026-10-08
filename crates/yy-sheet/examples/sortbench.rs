//! 保存した .yys を開いて並べ替え・絞り込みの速さを測る（大きな表で bench の一部だけを測り直す）。
//!
//! ```text
//! cargo run --release -p yy-sheet --example sortbench -- ファイル.yys
//! ```

use std::time::Instant;

use yy_sheet::query::{self, Cmp, ColFilter, Cond, SortKey};
use yy_sheet::{Context, yys};

fn main() {
    let path = std::env::args().nth(1).expect("ファイル");
    let ctx = Context::with_defaults();
    let doc = yys::open(ctx.clone(), path.as_ref()).unwrap();
    let table = &doc.book.sheets[0].table;
    println!("{} 行 × {} 列", table.rows, table.cols());
    for desc in [false, true] {
        let t = Instant::now();
        let o = query::sort(&ctx, table, &[SortKey { col: 0, desc }], None).unwrap();
        println!(
            "並べ替え（数値 1 キー・{}）{:.2} 秒 先頭 {}",
            if desc { "降順" } else { "昇順" },
            t.elapsed().as_secs_f64(),
            o[0]
        );
    }
    if table.cols() >= 2 {
        let t = Instant::now();
        query::sort(
            &ctx,
            table,
            &[
                SortKey { col: 1, desc: true },
                SortKey {
                    col: 0,
                    desc: false,
                },
            ],
            None,
        )
        .unwrap();
        println!("並べ替え（2 キー）{:.2} 秒", t.elapsed().as_secs_f64());
    }
    let t = Instant::now();
    let (_, counts) = query::filter(
        &ctx,
        table,
        &[ColFilter {
            col: 0,
            cond: Cond::Number {
                op: Cmp::Lt,
                value: 500_000.0,
            },
        }],
    )
    .unwrap();
    println!("絞り込み {:.2} 秒 → {counts:?}", t.elapsed().as_secs_f64());
}
