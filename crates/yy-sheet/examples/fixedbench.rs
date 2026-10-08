//! 固定長ファイルの取り込み・書き出しの速さ。
//!
//! ```text
//! cargo run --release -p yy-sheet --example fixedbench -- 行数 作業フォルダ
//! ```

use std::time::Instant;

use yy_cobol::{Charset, Codec, Input, Issues};
use yy_encoding::Ccsid;
use yy_sheet::Context;
use yy_sheet::fixed::{self, FixedSpec, RecordSep};

const COPY: &str = "
       01  REC.
           05  ID        PIC 9(8).
           05  NAME      PIC X(20).
           05  KANA      PIC N(10).
           05  AMT       PIC S9(9)V99 COMP-3.
           05  QTY       PIC S9(9) COMP.
           05  RATE      PIC S9(3)V9(4).
           05  EDIT      PIC ZZ,ZZZ,ZZ9.99-.
           05  CODE      PIC X(4).
";

fn main() {
    let mut args = std::env::args().skip(1);
    let rows: u64 = args
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1_000_000);
    let dir = std::path::PathBuf::from(args.next().unwrap_or_else(|| ".".into()));
    std::fs::create_dir_all(&dir).unwrap();
    let ctx = Context::with_defaults();
    let spec = FixedSpec::new(
        COPY,
        Codec::new(Charset::Ebcdic(Ccsid::Ibm930)),
        RecordSep::None,
    )
    .unwrap();
    let reclen = spec.layout.record_len;
    let path = dir.join("bench.dat");
    let t = Instant::now();
    {
        use std::io::Write;
        let mut out = std::io::BufWriter::new(std::fs::File::create(&path).unwrap());
        let mut rec = vec![0u8; reclen];
        let mut is = Issues::default();
        let names = ["山田太郎", "SUZUKI", "佐藤花子", "TANAKA ICHIRO"];
        for i in 0..rows {
            for f in &spec.layout.fields {
                let v = match f.name.as_str() {
                    "ID" => Input::Number(i as f64),
                    "NAME" => Input::Text(names[(i % 4) as usize]),
                    "KANA" => Input::Text("カタカナ"),
                    "AMT" => Input::Number((i % 100_000) as f64 * 1.25 - 5000.0),
                    "QTY" => Input::Number((i % 977) as f64),
                    "RATE" => Input::Number(0.1234),
                    "EDIT" => Input::Number(-(i as f64) / 4.0),
                    _ => Input::Text("A1"),
                };
                spec.codec
                    .encode(f, v, &mut rec[f.offset..f.offset + f.len], &mut is);
            }
            out.write_all(&rec).unwrap();
        }
    }
    let mb = (rows * reclen as u64) as f64 / 1e6;
    println!(
        "作成 {rows} レコード × {reclen} バイト = {mb:.0} MB（{:.1} 秒）",
        t.elapsed().as_secs_f64()
    );
    let t = Instant::now();
    let (sheet, rep) = fixed::import(&ctx, &path, &spec, &|_, _| true).unwrap();
    let s = t.elapsed().as_secs_f64();
    println!(
        "取り込み {s:.2} 秒（{:.0} MB/s）{} レコード 不正 {}",
        mb / s,
        rep.records,
        rep.invalid
    );
    let out = dir.join("out.dat");
    let t = Instant::now();
    let rep = fixed::export(&ctx, &sheet, &out, &spec, None, &|_, _| true).unwrap();
    let s = t.elapsed().as_secs_f64();
    println!(
        "書き出し {s:.2} 秒（{:.0} MB/s）注意 {}",
        mb / s,
        rep.issues.total()
    );
    let same = std::fs::read(&path).unwrap() == std::fs::read(&out).unwrap();
    println!("元と同じバイト列: {same}");
    let ms = FixedSpec {
        codec: Codec::new(Charset::Ms932),
        separator: RecordSep::Crlf,
        ..spec.clone()
    };
    let t = Instant::now();
    fixed::export(&ctx, &sheet, &dir.join("ms932.dat"), &ms, None, &|_, _| {
        true
    })
    .unwrap();
    println!("MS932 へ書き出し {:.2} 秒", t.elapsed().as_secs_f64());
    for p in ["bench.dat", "out.dat", "ms932.dat"] {
        let _ = std::fs::remove_file(dir.join(p));
    }
}
