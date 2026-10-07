//! `CBL.MOVE` の送り方（`Codec::move_elementary`・`move_group`）を GnuCOBOL の MOVE と比べる。
//!
//!   cargo run -p yy-cobol --example gnucobol_moves -- <作業フォルダ>
//!
//! `tests/gnucobol/moves.cob` を cobc（-std=ibm。2 進数は IBM の既定の TRUNC(STD) と同じく PIC の桁で
//! 切るよう -fbinary-truncate）で作って動かし、COBOL が書いた受け取りの項目の
//! バイト列と、同じ MOVE を yy-cobol で行ったバイト列を比べる（文字コードは MS932〔ASCII〕）。

use std::path::Path;
use std::process::Command;

use yy_cobol::{Charset, Codec, Field, Input, parse};

fn field(pic: &str) -> Field {
    parse(&format!("01 R.\n 05 F {pic}.\n"))
        .unwrap()
        .fields
        .remove(0)
}

fn main() {
    let work = std::env::args().nth(1).expect("作業フォルダ");
    let work = Path::new(&work);
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/gnucobol/moves.cob");
    let exe = work.join("moves");
    let out = work.join("moves.dat");
    let st = Command::new("cobc")
        .args(["-x", "-std=ibm", "-fbinary-truncate", "-o"])
        .arg(&exe)
        .arg(&src)
        .status()
        .expect("cobc");
    assert!(st.success(), "cobc に失敗しました");
    let st = Command::new(&exe)
        .current_dir(work)
        .status()
        .expect("moves");
    assert!(st.success(), "moves の実行に失敗しました");
    let cobol = std::fs::read(&out).expect("moves.dat");

    let c = Codec::new(Charset::Ms932);
    let e = |v: Input<'_>, s: &str, d: &str| -> Vec<u8> {
        c.move_elementary(v, Some(&field(s)), &field(d))
            .unwrap_or_else(|| panic!("{s} → {d} を送れません"))
            .1
    };
    let mut ours: Vec<(&str, Vec<u8>)> = vec![
        (
            "S9(5)V99 → 9(3)",
            e(Input::Number(12345.67), "PIC S9(5)V99", "PIC 9(3)"),
        ),
        (
            "S9(5)V99 → 9(3) 負",
            e(Input::Number(-7.5), "PIC S9(5)V99", "PIC 9(3)"),
        ),
        (
            "9(5) → X(8)",
            e(Input::Number(42.0), "PIC 9(5)", "PIC X(8)"),
        ),
        (
            "S9(3) → X(8)",
            e(Input::Number(-42.0), "PIC S9(3)", "PIC X(8)"),
        ),
        (
            "9(9) → X(4)",
            e(Input::Number(123456789.0), "PIC 9(9)", "PIC X(4)"),
        ),
        (
            "編集 → X(12)",
            e(Input::Number(-1234.5), "PIC ZZ,ZZ9.99-", "PIC X(12)"),
        ),
        (
            "編集 → S9(5)V99",
            e(Input::Number(-1234.5), "PIC ZZ,ZZ9.99-", "PIC S9(5)V99"),
        ),
        (
            "X(5) → S9(5)V99",
            e(Input::Text("00123"), "PIC X(5)", "PIC S9(5)V99"),
        ),
        (
            "X → JUSTIFIED",
            e(Input::Text("AB"), "PIC X(2)", "PIC X(5) JUSTIFIED RIGHT"),
        ),
    ];
    let (id, nm, amt) = (
        field("PIC 9(3)"),
        field("PIC X(4)"),
        field("PIC S9(3)V99 COMP-3"),
    );
    let src_row = [
        (Input::Number(7.0), Some(&id)),
        (Input::Text("AB"), Some(&nm)),
        (Input::Number(-1.5), Some(&amt)),
    ];
    let wide = field("PIC X(10)");
    ours.push((
        "集団 → X(10)",
        c.move_group(&src_row, &[&wide])[0].1.clone(),
    ));
    let g = c.move_group(&[(Input::Text("12345XY"), Some(&wide))], &[&id, &nm]);
    ours.push(("X(10) → 集団", [g[0].1.clone(), g[1].1.clone()].concat()));
    ours.push((
        "S9(5)V99 → S9(4) COMP",
        e(Input::Number(-12345.67), "PIC S9(5)V99", "PIC S9(4) COMP"),
    ));
    ours.push((
        "S9(5)V99 → S9(3)V99 COMP-3",
        e(
            Input::Number(9876.54),
            "PIC S9(5)V99",
            "PIC S9(3)V99 COMP-3",
        ),
    ));
    ours.push((
        "S9(5)V99 → 編集",
        e(Input::Number(-0.5), "PIC S9(5)V99", "PIC ZZ,ZZ9.99-"),
    ));
    ours.push((
        "9(9) → 9(5) COMP-3",
        e(Input::Number(123456789.0), "PIC 9(9)", "PIC 9(5) COMP-3"),
    ));
    let (a, b) = (field("PIC 9(3)"), field("PIC S9(3) COMP-3"));
    let g = c.move_group(&src_row, &[&a, &b]);
    ours.push((
        "集団 → 集団（短い）",
        [g[0].1.clone(), g[1].1.clone()].concat(),
    ));

    let mut at = 0;
    let mut bad = 0;
    for (name, b) in &ours {
        let theirs = cobol.get(at..at + b.len()).unwrap_or_default();
        if theirs == b.as_slice() {
            println!("  一致 {name}: {}", yy_cobol::hex_text(b));
        } else {
            bad += 1;
            println!(
                "  不一致 {name}: COBOL {} / yy-cobol {}",
                yy_cobol::hex_text(theirs),
                yy_cobol::hex_text(b)
            );
        }
        at += b.len();
    }
    // 行順編成の 1 レコード（改行を付けることがある）
    let total: usize = ours.iter().map(|o| o.1.len()).sum();
    if cobol.len() < total {
        eprintln!("COBOL のレコードが短い（{} < {total} バイト）", cobol.len());
        std::process::exit(1);
    }
    if bad > 0 {
        eprintln!("MOVE: {bad} 件が一致しません");
        std::process::exit(1);
    }
    println!("MOVE（CBL.MOVE）: {} 件 → すべて一致", ours.len());
}
