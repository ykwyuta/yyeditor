//! yy-cobol の解釈を GnuCOBOL（`cobc -std=ibm`）と比べる。
//!
//! ```text
//! cargo run -p yy-cobol --example gnucobol -- 作業フォルダ
//! ```
//!
//! コピーブック（`tests/gnucobol/*.cpy`）ごとに、同じコピーブックを `COPY` する COBOL のプログラムを作り:
//!
//! 1. **レイアウト**: 各基本項目の位置（`ADDRESS OF` の差）と長さ（`LENGTH OF`）を COBOL に出させ、
//!    yy-cobol のレイアウトと比べる。
//! 2. **COBOL → yy-cobol**: COBOL が `MOVE` して `WRITE` したファイルを yy-cobol で読み、期待の値と比べる。
//! 3. **yy-cobol → バイト**: 同じ値を yy-cobol で書き、COBOL のファイルとバイト単位で比べる。
//! 4. **yy-cobol → COBOL**: yy-cobol で書いたファイルを COBOL で読み、各項目を期待の値と比べる。
//!
//! 文字コードは MS932（GnuCOBOL は ASCII で動くので、ゾーン 10 進数の負の符号は `70`〜`79`）。
//! どれかが合わなければ 0 以外で終わる。

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use yy_cobol::{Charset, Codec, Decimal, Decoded, Field, Input, Issues, Kind, Layout};
use yy_encoding::Ccsid;

/// 1 つのコピーブックの試験。
struct Case {
    copybook: &'static str,
    /// 2 進数・浮動小数点をリトルエンディアンで（COMP-5・COMP-1・COMP-2 は機械の並び）
    little_endian: bool,
    /// ゾーン 10 進数の符号を EBCDIC の形で書かせ（`-fsign=EBCDIC`）、iconv でこれらの EBCDIC に
    /// して比べる（iconv の名前, CCSID）。空なら MS932（ASCII）で比べる
    ebcdic: &'static [(&'static str, Ccsid)],
    /// レコードごとの（項目, 値）。書いていない項目は 0 か空白
    records: Vec<Vec<(&'static str, &'static str)>>,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            copybook: "rec1.cpy",
            little_endian: false,
            ebcdic: &[],
            records: vec![
                vec![
                    ("T-ID", "1"),
                    ("T-NAME", "HELLO WORLD"),
                    ("T-ALPHA", "ABCDE"),
                    ("T-JUST", "12"),
                    ("T-STAT", "A"),
                    ("T-Z1", "12345.67"),
                    ("T-Z2", "7"),
                    ("T-Z3", "123"),
                    ("T-Z4", "5"),
                    ("T-Z5", "12"),
                    ("T-Z6", "123456789012345.678"),
                    ("T-P1", "1234567.89"),
                    ("T-P2", "12"),
                    ("T-P3", "123456789012345678"),
                    ("T-P4", "1.5"),
                    ("T-B1", "9999"),
                    ("T-B2", "300"),
                    ("T-B3", "123456789012345678"),
                    ("T-B4", "12345.67"),
                    ("T-E1", "1234.5"),
                    ("T-E2", "12345.67"),
                    ("T-E3", "5"),
                    ("T-E4", "123"),
                    ("T-E5", "20261007"),
                    ("T-E6", "42"),
                    ("T-E7", "-7"),
                    ("T-E8", "123456"),
                    ("T-E9", "-1.5"),
                    ("T-PS", "12000"),
                    ("T-VP", "0.0012"),
                    ("T-OA(1)", "A1"),
                    ("T-OB(1)", "1"),
                    ("T-OA(2)", "B2"),
                    ("T-OB(2)", "-2"),
                    ("T-OA(3)", "C3"),
                    ("T-OB(3)", "999"),
                    ("T-RED-SRC", "ABCDEFGH"),
                    ("T-PAD", "Z"),
                    ("T-SYNC", "123456789"),
                    ("T-U1", "-12"),
                    ("T-U2", "34"),
                    ("T-S1", "-5"),
                ],
                vec![
                    ("T-ID", "2"),
                    ("T-NAME", "ABC"),
                    ("T-ALPHA", "XYZ"),
                    ("T-JUST", "ABCDEF"),
                    ("T-Z1", "-0.01"),
                    ("T-Z2", "999"),
                    ("T-Z3", "-45"),
                    ("T-Z4", "-999"),
                    ("T-Z5", "-1"),
                    ("T-Z6", "-1.5"),
                    ("T-P1", "-1234.5"),
                    ("T-P2", "9999"),
                    ("T-P3", "-987654321098765432"),
                    ("T-P4", "-999.999"),
                    ("T-B1", "-2"),
                    ("T-B2", "99999999"),
                    ("T-B3", "-1"),
                    ("T-B4", "-0.5"),
                    ("T-E1", "-12.5"),
                    ("T-E2", "-1.5"),
                    ("T-E3", "123.45"),
                    ("T-E4", "-12"),
                    ("T-E5", "19991231"),
                    ("T-E6", "7"),
                    ("T-E7", "1234"),
                    ("T-E8", "1"),
                    ("T-E9", "2.25"),
                    ("T-PS", "99000"),
                    ("T-VP", "-0.0099"),
                    ("T-OB(1)", "-999"),
                    ("T-SYNC", "-1"),
                    ("T-U1", "999"),
                    ("T-S1", "12"),
                ],
                // 0 と空白
                vec![("T-ID", "9999")],
                // 小数部の多い桁は切り捨て（COBOL の MOVE）
                vec![
                    ("T-ID", "4"),
                    ("T-Z1", "1.239"),
                    ("T-Z6", "-0.0009"),
                    ("T-P1", "-0.019"),
                    ("T-P4", "-1.2345"),
                    ("T-B4", "1.999"),
                    ("T-E1", "0.999"),
                    ("T-E2", "-0.005"),
                    ("T-E9", "9.999"),
                    ("T-PS", "12999"),
                    ("T-VP", "0.00129"),
                ],
            ],
        },
        Case {
            copybook: "rec2.cpy",
            little_endian: true,
            ebcdic: &[],
            records: vec![
                vec![
                    ("N-ID", "1"),
                    ("N-C5A", "30000"),
                    ("N-C5B", "-123456789"),
                    ("N-C5C", "123456789012345678"),
                    ("N-F1", "1.5"),
                    ("N-F2", "3.25"),
                ],
                vec![
                    ("N-ID", "2"),
                    ("N-C5A", "-32768"),
                    ("N-C5B", "2147483647"),
                    ("N-F1", "-118.625"),
                    ("N-F2", "0.1"),
                ],
            ],
        },
        Case {
            copybook: "rec3.cpy",
            little_endian: false,
            ebcdic: &[("IBM037", Ccsid::Ibm037), ("IBM1047", Ccsid::Ibm1047)],
            records: vec![
                vec![
                    ("E-ID", "1"),
                    ("E-NAME", "HELLO 123"),
                    ("E-Z1", "12345.67"),
                    ("E-Z2", "123"),
                    ("E-Z3", "45"),
                    ("E-Z4", "6"),
                    ("E-Z5", "99999"),
                    ("E-E1", "1234.5"),
                    ("E-E2", "12.5"),
                    ("E-E3", "7"),
                ],
                vec![
                    ("E-ID", "2"),
                    ("E-NAME", "A-B/C.D,E*"),
                    ("E-Z1", "-12345.67"),
                    ("E-Z2", "-1"),
                    ("E-Z3", "-45"),
                    ("E-Z4", "-999"),
                    ("E-Z5", "1"),
                    ("E-E1", "-0.5"),
                    ("E-E2", "-9.99"),
                    ("E-E3", "-12"),
                ],
                vec![("E-ID", "3"), ("E-Z1", "-0.01")],
            ],
        },
    ]
}

/// GnuCOBOL の側の違いとわかっているもの（コピーブック, レコード, 項目, 理由）。不一致に数えず、
/// 参考として出す。
const KNOWN: &[(&str, usize, &str, &str)] = &[
    (
        "rec2.cpy",
        2,
        "N-F2",
        "GnuCOBOL 3.1 は 0.1 を最も近い倍精度（3FB999999999999A）でなく 1 つ下（…99）にする。\
         yy-cobol は最も近い値（IEEE 754 の丸め）で書く",
    ),
    (
        "rec1.cpy",
        4,
        "T-Z6",
        "-0.0009 を小数部 3 桁に切り捨てると 0 になる。GnuCOBOL 3.1 はゾーン 10 進数に負の 0（符号 70）を\
         書き、yy-cobol は正の 0 を書く（数値はどちらも 0。yy-cobol は負の 0 も 0 と読む）",
    ),
    (
        "rec1.cpy",
        1,
        "T-E9",
        "GnuCOBOL 3.1 の編集の解除（MOVE 編集項目 TO 数字項目）は DB を負と見ない（CR は見る）。\
         バイト列と yy-cobol の読みは一致している",
    ),
];

fn known(copybook: &str, rec: usize, field: &str) -> Option<&'static str> {
    KNOWN
        .iter()
        .find(|k| k.0 == copybook && k.1 == rec && k.2 == field)
        .map(|k| k.3)
}

/// 項目の値（書いていなければ 0 か空白）。
fn value_of<'a>(rec: &HashMap<&str, &'a str>, f: &Field) -> &'a str {
    rec.get(f.name.as_str())
        .copied()
        .unwrap_or(if f.kind.is_numeric() { "0" } else { "" })
}

fn is_filler(f: &Field) -> bool {
    f.name.starts_with("FILLER")
}

/// COBOL が持つはずの値（数値の項目は、多い桁を切り捨てた値）。
fn expected(f: &Field, v: &str) -> String {
    match f.kind.digits_scale() {
        Some((_, scale)) => Decimal::parse(v)
            .and_then(|d| d.truncate(scale))
            .map(|d| d.to_string())
            .unwrap_or_else(|| v.to_string()),
        None => v.to_string(),
    }
}

/// COBOL の定数（`JUSTIFIED RIGHT` の項目と比べるときは右に寄せる: 比べるときは定数の右に空白を
/// 足すため）。
fn literal(f: &Field, v: &str, compare: bool) -> String {
    if f.kind.is_numeric() {
        v.to_string()
    } else if v.is_empty() {
        "SPACES".into()
    } else if compare
        && matches!(
            f.kind,
            Kind::Alnum {
                justified: true,
                ..
            }
        )
    {
        format!("'{:>w$}'", v.replace('\'', "''"), w = f.len)
    } else {
        format!("'{}'", v.replace('\'', "''"))
    }
}

/// 固定形式の 1 文（72 桁に収める）。
fn stmt(out: &mut String, s: &str) {
    if 11 + s.len() <= 72 {
        let _ = writeln!(out, "           {s}");
        return;
    }
    // 語の境目で折り返す
    let mut line = String::new();
    for w in s.split(' ') {
        if 11 + line.len() + w.len() + 1 > 72 {
            let _ = writeln!(out, "           {line}");
            line.clear();
            line.push_str("    ");
        }
        if !line.trim().is_empty() {
            line.push(' ');
        }
        line.push_str(w);
    }
    let _ = writeln!(out, "           {line}");
}

fn header(out: &mut String, id: &str, file: &str, copybook: &Path) {
    let _ = write!(
        out,
        "       IDENTIFICATION DIVISION.
       PROGRAM-ID. {id}.
       ENVIRONMENT DIVISION.
       INPUT-OUTPUT SECTION.
       FILE-CONTROL.
           SELECT DATA-F ASSIGN TO \"{file}\"
               ORGANIZATION IS SEQUENTIAL.
       DATA DIVISION.
       FILE SECTION.
       FD  DATA-F.
           COPY \"{}\".
       WORKING-STORAGE SECTION.
       01  WS-P USAGE POINTER.
       01  WS-PN REDEFINES WS-P PIC 9(18) COMP-5.
       01  WS-B PIC 9(18) COMP-5.
       01  WS-O PIC 9(9).
       01  WS-DE PIC S9(18)V9(9).
       PROCEDURE DIVISION.
",
        copybook.file_name().unwrap().to_string_lossy()
    );
}

/// COBOL のプログラム（`GEN`: レイアウトを出して書く、`CHK`: 読んで比べる）。
fn programs(c: &Case, l: &Layout, copybook: &Path) -> (String, String) {
    let fields: Vec<&Field> = l.fields.iter().filter(|f| !is_filler(f)).collect();
    let mut g = String::new();
    header(&mut g, "GEN", "cobol.dat", copybook);
    stmt(&mut g, "OPEN OUTPUT DATA-F.");
    stmt(&mut g, &format!("SET WS-P TO ADDRESS OF {}.", l.record));
    stmt(&mut g, "MOVE WS-PN TO WS-B.");
    stmt(&mut g, &format!("DISPLAY \"R \" LENGTH OF {}.", l.record));
    for f in &fields {
        stmt(&mut g, &format!("SET WS-P TO ADDRESS OF {}.", f.name));
        stmt(&mut g, "COMPUTE WS-O = WS-PN - WS-B.");
        stmt(
            &mut g,
            &format!("DISPLAY \"L {} \" WS-O \" \" LENGTH OF {}.", f.name, f.name),
        );
    }
    for r in &c.records {
        let m: HashMap<&str, &str> = r.iter().copied().collect();
        stmt(&mut g, &format!("MOVE SPACES TO {}.", l.record));
        for f in &fields {
            stmt(
                &mut g,
                &format!("MOVE {} TO {}.", literal(f, value_of(&m, f), false), f.name),
            );
        }
        stmt(&mut g, &format!("WRITE {}.", l.record));
    }
    stmt(&mut g, "CLOSE DATA-F.");
    stmt(&mut g, "STOP RUN.");

    let mut k = String::new();
    header(&mut k, "CHK", "rust.dat", copybook);
    stmt(&mut k, "OPEN INPUT DATA-F.");
    for (i, r) in c.records.iter().enumerate() {
        let m: HashMap<&str, &str> = r.iter().copied().collect();
        stmt(&mut k, "READ DATA-F AT END DISPLAY \"NG EOF\" END-READ.");
        for f in &fields {
            let v = expected(f, value_of(&m, f));
            let v = v.as_str();
            let ng = format!(
                "DISPLAY \"NG {} {} [\" {} \"]\" END-IF.",
                i + 1,
                f.name,
                f.name
            );
            if matches!(f.kind, Kind::Edited { .. }) {
                stmt(&mut k, &format!("MOVE {} TO WS-DE.", f.name));
                stmt(&mut k, &format!("IF WS-DE NOT = {v}"));
            } else {
                stmt(
                    &mut k,
                    &format!("IF {} NOT = {}", f.name, literal(f, v, true)),
                );
            }
            stmt(&mut k, &ng);
        }
    }
    stmt(&mut k, &format!("DISPLAY \"CHECKED {}\".", c.records.len()));
    stmt(&mut k, "CLOSE DATA-F.");
    stmt(&mut k, "STOP RUN.");
    (g, k)
}

fn hex(b: &[u8]) -> String {
    b.iter()
        .map(|x| format!("{x:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// 読んだ値が期待の値と同じか。
fn same(f: &Field, d: &Decoded, v: &str) -> bool {
    match (&f.kind, d) {
        (Kind::Float { double }, Decoded::Float(x)) => {
            let want: f64 = v.parse().unwrap_or(f64::NAN);
            let want = if *double { want } else { want as f32 as f64 };
            *x == want
        }
        (k, Decoded::Num(n)) if k.is_numeric() => {
            let scale = k.digits_scale().map(|x| x.1).unwrap_or(0);
            // COBOL の MOVE と同じく、多い桁は切り捨てた値
            Decimal::parse(v).and_then(|e| e.truncate(scale)) == n.rescale(scale)
        }
        (k, Decoded::Empty) if k.is_numeric() => Decimal::parse(v).is_some_and(|e| e.value == 0),
        (_, Decoded::Empty) => v.is_empty(),
        (_, Decoded::Text(t)) => t == v.trim_end(),
        _ => false,
    }
}

fn run(cmd: &mut Command) -> Result<String, String> {
    let out = cmd.output().map_err(|e| format!("{cmd:?}: {e}"))?;
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    if !out.status.success() {
        return Err(format!("{cmd:?} が失敗しました:\n{text}"));
    }
    Ok(text)
}

/// 結果（不一致・参考）。
#[derive(Default)]
struct Report {
    ng: Vec<String>,
    info: Vec<String>,
}

impl Report {
    /// 不一致（わかっている GnuCOBOL の側の違いなら参考に）。
    fn push(&mut self, copybook: &str, rec: usize, field: &str, msg: String) {
        match known(copybook, rec, field) {
            Some(why) => self.info.push(format!("{msg}\n      → {why}")),
            None => self.ng.push(msg),
        }
    }
}

/// COBOL のファイル（`data`）と yy-cobol の読み書きを比べ、yy-cobol で書いたバイト列を返す。
fn compare(
    c: &Case,
    l: &Layout,
    codec: &Codec,
    data: &[u8],
    tag: &str,
    pending: &mut Vec<(usize, String, String)>,
    rep: &mut Report,
) -> Vec<u8> {
    let space = codec.charset.space();
    let mut rust = vec![0u8; data.len()];
    let mut issues = Issues::default();
    for (i, r) in c.records.iter().enumerate() {
        let m: HashMap<&str, &str> = r.iter().copied().collect();
        let rec = &data[i * l.record_len..(i + 1) * l.record_len];
        let out = &mut rust[i * l.record_len..(i + 1) * l.record_len];
        // 項目のない所（FILLER・SYNC の埋め草）は COBOL と同じく空白
        out.fill(space);
        for f in &l.fields {
            let raw = &rec[f.offset..f.offset + f.len];
            let v = value_of(&m, f);
            if !is_filler(f) {
                let d = codec.decode(f, raw);
                if !same(f, &d, v) {
                    let msg = format!(
                        "{tag}読み {} {}: 期待 {v:?}、yy-cobol {d:?}（バイト {}）",
                        i + 1,
                        f.name,
                        hex(raw)
                    );
                    pending.push((i + 1, f.name.clone(), msg));
                }
            }
            // yy-cobol で書く
            let input = if v.is_empty() && !f.kind.is_numeric() {
                Input::Empty
            } else {
                Input::Text(v)
            };
            let o = &mut out[f.offset..f.offset + f.len];
            codec.encode(f, input, o, &mut issues);
            if o != raw {
                let msg = format!(
                    "{tag}書き {} {}: COBOL {}、yy-cobol {}",
                    i + 1,
                    f.name,
                    hex(raw),
                    hex(o)
                );
                pending.push((i + 1, f.name.clone(), msg));
            }
        }
    }
    if issues.total() > 0 {
        rep.ng
            .push(format!("{tag}yy-cobol が書くときの注意: {issues:?}"));
    }
    let wrote_ng = pending.iter().any(|p| p.2.contains("書き"));
    if rust != data
        && !wrote_ng
        && let Some(at) = rust.iter().zip(data).position(|(a, b)| a != b)
    {
        rep.ng
            .push(format!("{tag}バイト {at} が違います（項目の外）"));
    }
    rust
}

/// COBOL で読んで比べる（`./chk` は `rust.dat` を読む）。
fn cobol_read(
    c: &Case,
    work: &Path,
    tag: &str,
    pending: &mut Vec<(usize, String, String)>,
    rep: &mut Report,
) -> Result<(), String> {
    let out = run(Command::new("./chk").current_dir(work))?;
    for line in out.lines() {
        if line.starts_with("NG") {
            let w: Vec<&str> = line.split_whitespace().collect();
            let rec = w.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
            let field = w.get(2).copied().unwrap_or("").to_string();
            pending.push((rec, field, format!("{tag}COBOL で読み: {line}")));
        }
    }
    if !out.contains(&format!("CHECKED {}", c.records.len())) {
        rep.ng
            .push(format!("{tag}COBOL の読みが終わりませんでした:\n{out}"));
    }
    Ok(())
}

/// iconv でファイルの文字コードを変える。
fn iconv(from: &str, to: &str, src: &Path, dst: &Path) -> Result<(), String> {
    let out = Command::new("iconv")
        .args(["-f", from, "-t", to])
        .arg(src)
        .output()
        .map_err(|e| format!("iconv: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "iconv -f {from} -t {to} が失敗しました: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    std::fs::write(dst, out.stdout).map_err(|e| e.to_string())
}

fn check(c: &Case, dir: &Path, src: &Path) -> Result<Report, String> {
    let mut rep = Report::default();
    let text = std::fs::read_to_string(src.join(c.copybook)).map_err(|e| e.to_string())?;
    let l = yy_cobol::parse(&text)?;
    let work = dir.join(c.copybook.trim_end_matches(".cpy"));
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    std::fs::copy(src.join(c.copybook), work.join(c.copybook)).map_err(|e| e.to_string())?;
    let (gen_src, chk_src) = programs(c, &l, &work.join(c.copybook));
    std::fs::write(work.join("gen.cbl"), &gen_src).map_err(|e| e.to_string())?;
    std::fs::write(work.join("chk.cbl"), &chk_src).map_err(|e| e.to_string())?;
    let sign = if c.ebcdic.is_empty() {
        "-fsign=ASCII"
    } else {
        "-fsign=EBCDIC"
    };
    for p in ["gen", "chk"] {
        run(Command::new("cobc")
            .args(["-x", "-std=ibm", sign, "-o", p, &format!("{p}.cbl")])
            .current_dir(&work))?;
    }
    // 1. レイアウト
    let out = run(Command::new("./gen").current_dir(&work))?;
    let mut cob: HashMap<String, (usize, usize)> = HashMap::new();
    let mut reclen = 0;
    for line in out.lines() {
        let w: Vec<&str> = line.split_whitespace().collect();
        match w.as_slice() {
            ["R", n] => reclen = n.parse().unwrap_or(0),
            ["L", name, off, len] => {
                cob.insert(
                    name.to_string(),
                    (off.parse().unwrap_or(usize::MAX), len.parse().unwrap_or(0)),
                );
            }
            _ => {}
        }
    }
    if reclen != l.record_len {
        rep.ng.push(format!(
            "レコード長: COBOL {reclen}、yy-cobol {}",
            l.record_len
        ));
    }
    for f in l.fields.iter().filter(|f| !is_filler(f)) {
        match cob.get(&f.name) {
            Some(&(o, n)) if (o, n) == (f.offset, f.len) => {}
            Some(&(o, n)) => rep.ng.push(format!(
                "{}: 位置・長さ COBOL {}・{}、yy-cobol {}・{}",
                f.name, o, n, f.offset, f.len
            )),
            None => rep
                .ng
                .push(format!("{}: COBOL が位置を出していません", f.name)),
        }
    }
    let data = std::fs::read(work.join("cobol.dat")).map_err(|e| e.to_string())?;
    if data.len() != l.record_len * c.records.len() {
        rep.ng.push(format!(
            "COBOL のファイルの大きさ {}（{} × {} のはず）",
            data.len(),
            l.record_len,
            c.records.len()
        ));
        return Ok(rep);
    }
    let mut pending: Vec<(usize, String, String)> = Vec::new();
    if c.ebcdic.is_empty() {
        // 2・3. COBOL → yy-cobol、yy-cobol → バイト（MS932）
        let codec = Codec {
            charset: Charset::Ms932,
            little_endian: c.little_endian,
        };
        let rust = compare(c, &l, &codec, &data, "", &mut pending, &mut rep);
        std::fs::write(work.join("rust.dat"), &rust).map_err(|e| e.to_string())?;
        // 4. yy-cobol → COBOL
        cobol_read(c, &work, "", &mut pending, &mut rep)?;
    } else {
        for (name, ccsid) in c.ebcdic {
            let tag = format!("[{name}] ");
            let ebc = work.join(format!("cobol.{name}"));
            iconv("ISO-8859-1", name, &work.join("cobol.dat"), &ebc)?;
            let data = std::fs::read(&ebc).map_err(|e| e.to_string())?;
            let codec = Codec {
                charset: Charset::Ebcdic(*ccsid),
                little_endian: c.little_endian,
            };
            let rust = compare(c, &l, &codec, &data, &tag, &mut pending, &mut rep);
            let mine = work.join(format!("rust.{name}"));
            std::fs::write(&mine, &rust).map_err(|e| e.to_string())?;
            iconv(name, "ISO-8859-1", &mine, &work.join("rust.dat"))?;
            cobol_read(c, &work, &tag, &mut pending, &mut rep)?;
        }
    }
    for (r, f, msg) in pending {
        rep.push(c.copybook, r, &f, msg);
    }
    println!(
        "{}: レコード {} バイト・項目 {} 個・{} レコード{} → {}",
        c.copybook,
        l.record_len,
        l.fields.len(),
        c.records.len(),
        if c.ebcdic.is_empty() {
            "（MS932）".to_string()
        } else {
            format!(
                "（{}）",
                c.ebcdic.iter().map(|e| e.0).collect::<Vec<_>>().join("・")
            )
        },
        if rep.ng.is_empty() {
            "一致"
        } else {
            "不一致あり"
        }
    );
    Ok(rep)
}

/// EBCDIC の文字列の変換を iconv（glibc）と比べる（2 バイト文字・SO / SI を含む）。
fn text_vs_iconv() -> Vec<String> {
    let mut ng = Vec::new();
    let all: &[(&str, Ccsid, &[&str])] = &[
        (
            "IBM930",
            Ccsid::Ibm930,
            &[
                "ABC 123",
                "ｱｲｳｴｵ",
                "漢字テスト",
                "日本語ＡＢＣ",
                "A漢B字C",
                "￥１００",
            ],
        ),
        (
            "IBM939",
            Ccsid::Ibm939,
            &[
                "ABC 123 abc",
                "漢字テスト",
                "日本語ＡＢＣ",
                "A漢B字C",
                "ｱｲｳ",
            ],
        ),
        (
            "IBM1390",
            Ccsid::Ibm1390,
            &["ABC 123", "ｱｲｳｴｵ", "漢字テスト", "A漢B字C"],
        ),
        (
            "IBM1399",
            Ccsid::Ibm1399,
            &["ABC 123 abc", "漢字テスト", "A漢B字C"],
        ),
        ("IBM290", Ccsid::Ibm290, &["ABC 123", "ｱｲｳｴｵ"]),
        (
            "IBM037",
            Ccsid::Ibm037,
            &["Hello, World! 123", "a-b/c.d{e}"],
        ),
        ("IBM500", Ccsid::Ibm500, &["Hello, World! 123"]),
        ("IBM1047", Ccsid::Ibm1047, &["Hello, World! 123", "[x]^~"]),
    ];
    let mut checked = 0;
    for (name, ccsid, texts) in all {
        let codec = Codec::new(Charset::Ebcdic(*ccsid));
        for t in *texts {
            let out = Command::new("sh")
                .arg("-c")
                .arg(format!("printf '%s' \"$T\" | iconv -f UTF-8 -t {name}"))
                .env("T", t)
                .output();
            let Ok(out) = out else { continue };
            if !out.status.success() {
                println!("  （iconv が {name} に {t:?} を変換できないので飛ばします）");
                continue;
            }
            let want = out.stdout;
            let f = Field {
                name: "T".into(),
                offset: 0,
                len: want.len(),
                kind: Kind::Alnum {
                    justified: false,
                    alpha: false,
                },
                describe: String::new(),
            };
            let mut mine = vec![0u8; want.len()];
            let mut is = Issues::default();
            codec.encode(&f, Input::Text(t), &mut mine, &mut is);
            checked += 1;
            if mine != want || is.total() > 0 {
                ng.push(format!(
                    "文字列 {name} {t:?}: iconv {}、yy-cobol {}",
                    hex(&want),
                    hex(&mine)
                ));
            }
            let back = codec.decode(&f, &want);
            if back != Decoded::Text(t.to_string()) {
                ng.push(format!(
                    "文字列 {name} {t:?}: iconv のバイト列を yy-cobol で読むと {back:?}"
                ));
            }
        }
    }
    println!(
        "EBCDIC の文字列（iconv と比べる）: {checked} 個 → {}",
        if ng.is_empty() {
            "一致"
        } else {
            "不一致あり"
        }
    );
    ng
}

fn main() {
    let dir = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "gnucobol-work".into()),
    );
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/gnucobol");
    let version = run(Command::new("cobc").arg("--version")).unwrap_or_else(|e| {
        eprintln!("cobc が見つかりません（GnuCOBOL を入れてください）: {e}");
        std::process::exit(2);
    });
    println!("{}", version.lines().next().unwrap_or(""));
    let mut bad = 0;
    for c in cases() {
        match check(&c, &dir, &src) {
            Ok(rep) => {
                for n in &rep.ng {
                    println!("  NG {n}");
                }
                for n in &rep.info {
                    println!("  参考（GnuCOBOL の側の違い） {n}");
                }
                bad += rep.ng.len();
            }
            Err(e) => {
                println!("{}: {e}", c.copybook);
                bad += 1;
            }
        }
    }
    for n in text_vs_iconv() {
        println!("  NG {n}");
        bad += 1;
    }
    if bad > 0 {
        println!("不一致 {bad} 件");
        std::process::exit(1);
    }
    println!("すべて一致しました");
}
