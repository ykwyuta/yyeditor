use super::*;
use crate::value::Value;

fn opts(header: bool) -> CsvOptions {
    CsvOptions {
        dialect: Dialect::csv(),
        record_end: RecordEnd::Newline,
        encoding: Encoding::Utf8,
        header,
        types: Vec::new(),
        all_text: false,
        date_system: DateSystem::D1900,
    }
}

fn grid(ctx: &Context, s: &Sheet) -> Vec<Vec<Value>> {
    let (rows, cols) = s.extent();
    (0..rows)
        .map(|r| (0..cols).map(|c| s.get(ctx, r, c).unwrap()).collect())
        .collect()
}

fn t(s: &str) -> Value {
    Value::text(s)
}

fn n(x: f64) -> Value {
    Value::Number(x)
}

const OK: &(dyn Fn(u64, u64) -> bool + Sync) = &|_, _| true;

#[test]
fn imports_rfc4180_with_types() {
    let ctx = Context::for_tests();
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.csv");
    std::fs::write(
        &p,
        "\u{FEFF}地域,売上,日付,備考,有効\r\n\
         東京,\"1,200\",2026/10/7,\"改行を\r\n含む\",TRUE\r\n\
         大阪,300,2026-10-08,\"\"\"引用\"\"\",false\r\n\
         名古屋,,2026/10/9,00123,TRUE\r\n",
    )
    .unwrap();
    let pv = preview(&p).unwrap();
    assert_eq!(pv.options.encoding, Encoding::Utf8);
    assert!(pv.options.header);
    assert_eq!(
        pv.options.types,
        vec![
            ColType::Text,
            ColType::Number(Some("#,##0".into())),
            ColType::Number(Some("yyyy/m/d".into())),
            ColType::Text,
            ColType::Bool
        ]
    );
    let s = import(&ctx, &p, &pv.options, OK).unwrap();
    assert_eq!(s.table.rows, 3);
    let g = grid(&ctx, &s);
    assert_eq!(
        g[0],
        vec![t("地域"), t("売上"), t("日付"), t("備考"), t("有効")]
    );
    assert_eq!(
        g[1],
        vec![
            t("東京"),
            n(1200.0),
            n(46302.0),
            t("改行を\r\n含む"),
            Value::Bool(true)
        ]
    );
    assert_eq!(g[2][3], t("\"引用\""));
    assert_eq!(g[2][4], Value::Bool(false));
    assert_eq!(g[3][1], Value::Empty);
    assert_eq!(g[3][3], t("00123"));
    assert_eq!(s.table.columns[2].format.as_deref(), Some("yyyy/m/d"));
}

/// 簡単な乱数（試験用）。
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// 引用符・改行・区切り文字を含む乱数の CSV。
fn random_csv(rng: &mut Rng, rows: usize) -> String {
    let mut out = String::new();
    for r in 0..rows {
        let cols = 3 + rng.below(3) as usize;
        for c in 0..cols {
            if c > 0 {
                out.push(',');
            }
            match rng.below(6) {
                0 => out.push_str(&format!("{}", rng.below(100_000))),
                1 => out.push_str(&format!("\"x{r},\"\"{c}\"\"\ny\"")),
                2 => out.push_str("\"a\r\nb\nc\""),
                3 => {}
                4 => out.push_str(&format!("テキスト{}", rng.below(10))),
                _ => out.push_str(&format!("{}.5", rng.below(1000))),
            }
        }
        out.push_str(if rng.below(2) == 0 { "\n" } else { "\r\n" });
    }
    out
}

#[test]
fn parallel_sections_match_sequential() {
    let ctx = Context::for_tests();
    let dir = tempfile::tempdir().unwrap();
    for seed in 1..6u64 {
        let mut rng = Rng(seed * 7919);
        let text = random_csv(&mut rng, 400);
        let p = dir.path().join(format!("r{seed}.csv"));
        std::fs::write(&p, &text).unwrap();
        let mut o = opts(false);
        o.types = vec![ColType::Text; 6];
        let seq = import_with(&ctx, &p, &o, OK, Some(usize::MAX / 2)).unwrap();
        // 区画の境目が引用符の中に入る小さな区画
        for section in [17, 64, 333] {
            let par = import_with(&ctx, &p, &o, OK, Some(section)).unwrap();
            assert_eq!(
                par.table.rows, seq.table.rows,
                "seed {seed} section {section}"
            );
            assert_eq!(
                grid(&ctx, &par),
                grid(&ctx, &seq),
                "seed {seed} section {section}"
            );
        }
        assert_eq!(seq.table.rows, 400);
        // 引用符の中の改行を含む値
        let g = grid(&ctx, &seq);
        assert!(g.iter().flatten().any(|v| *v == t("a\r\nb\nc")));
    }
}

#[test]
fn ragged_rows_and_new_columns() {
    let ctx = Context::for_tests();
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("r.csv");
    let mut text = String::new();
    for i in 0..20 {
        text.push_str(&format!("{i}"));
        if i == 15 {
            text.push_str(",extra,more");
        }
        text.push('\n');
    }
    std::fs::write(&p, &text).unwrap();
    let s = import_with(&ctx, &p, &opts(false), OK, Some(10)).unwrap();
    assert_eq!(s.table.rows, 20);
    assert_eq!(s.table.cols(), 3);
    let g = grid(&ctx, &s);
    assert_eq!(g[15], vec![n(15.0), t("extra"), t("more")]);
    assert_eq!(g[3], vec![n(3.0), Value::Empty, Value::Empty]);
    assert_eq!(g[19], vec![n(19.0), Value::Empty, Value::Empty]);
}

#[test]
fn shift_jis_and_utf16() {
    let ctx = Context::for_tests();
    let dir = tempfile::tempdir().unwrap();
    let text = "名前,数量\r\nソフト表示,1\r\n\"能力,予定\",2\r\n";
    for enc in [Encoding::Cp932, Encoding::Utf16Le] {
        let p = dir.path().join(format!("{}.csv", enc.name()));
        let mut bytes = enc.bom().to_vec();
        bytes.extend(yy_encoding::encode_all(enc, text.as_bytes(), EscapeMode::Literal).unwrap());
        std::fs::write(&p, &bytes).unwrap();
        let pv = preview(&p).unwrap();
        assert!(
            pv.options.encoding.same_charset(&enc),
            "{:?}",
            pv.options.encoding
        );
        let s = import(&ctx, &p, &pv.options, OK).unwrap();
        let g = grid(&ctx, &s);
        assert_eq!(g[0], vec![t("名前"), t("数量")], "{enc:?}");
        assert_eq!(g[1], vec![t("ソフト表示"), n(1.0)]);
        assert_eq!(g[2], vec![t("能力,予定"), n(2.0)]);
    }
}

#[test]
fn export_round_trip() {
    let ctx = Context::for_tests();
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("s.csv");
    std::fs::write(
        &src,
        "名前,値,日付\n\"a,b\",1.5,2026/10/7\n\"改\n行\",-2,2026/1/2\n\"q\"\"q\",,2026/3/4\n",
    )
    .unwrap();
    let pv = preview(&src).unwrap();
    let s = import(&ctx, &src, &pv.options, OK).unwrap();
    let out = dir.path().join("o.csv");
    let raw = ExportOptions {
        formatted: false,
        crlf: false,
        ..ExportOptions::default()
    };
    let rep = export(&ctx, &s, DateSystem::D1900, &out, &raw, None, OK).unwrap();
    assert_eq!(rep.rows, 4);
    assert_eq!(
        std::fs::read_to_string(&out).unwrap(),
        "名前,値,日付\n\"a,b\",1.5,2026-10-07\n\"改\n行\",-2,2026-01-02\n\"q\"\"q\",,2026-03-04\n"
    );
    let back = import(&ctx, &out, &preview(&out).unwrap().options, OK).unwrap();
    assert_eq!(grid(&ctx, &back), grid(&ctx, &s));
    // 並べ替えの順で、Shift_JIS・CRLF で
    let sj = ExportOptions {
        encoding: Encoding::Cp932,
        formatted: false,
        ..ExportOptions::default()
    };
    export(&ctx, &s, DateSystem::D1900, &out, &sj, Some(&[2, 0]), OK).unwrap();
    let bytes = std::fs::read(&out).unwrap();
    let (text, _) = yy_encoding::decode_all(Encoding::Cp932, &bytes, false);
    assert_eq!(
        String::from_utf8(text).unwrap(),
        "名前,値,日付\r\n\"q\"\"q\",,2026-03-04\r\n\"a,b\",1.5,2026-10-07\r\n"
    );
    // 書式の表示形式で書く（列全体は速い道、一部の範囲は格子をたどる）
    let fmt = |f: &str| crate::style::Style {
        num_fmt: Some(std::sync::Arc::from(f)),
        ..Default::default()
    };
    let mut s2 = s.clone();
    s2.styles
        .set(crate::style::Rect::new(0, 1, u64::MAX, 1), fmt("0.00"));
    let shown = ExportOptions {
        crlf: false,
        ..ExportOptions::default()
    };
    export(&ctx, &s2, DateSystem::D1900, &out, &shown, None, OK).unwrap();
    assert_eq!(
        std::fs::read_to_string(&out).unwrap(),
        "名前,値,日付\n\"a,b\",1.50,2026/10/7\n\"改\n行\",-2.00,2026/1/2\n\"q\"\"q\",,2026/3/4\n"
    );
    s2.styles
        .set(crate::style::Rect::new(2, 2, 2, 2), fmt("yyyy年m月d日"));
    export(&ctx, &s2, DateSystem::D1900, &out, &shown, None, OK).unwrap();
    assert_eq!(
        std::fs::read_to_string(&out).unwrap(),
        "名前,値,日付\n\"a,b\",1.50,2026/10/7\n\"改\n行\",-2.00,2026年1月2日\n\"q\"\"q\",,2026/3/4\n"
    );
}

#[test]
fn cancel_stops_import() {
    let ctx = Context::for_tests();
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("c.csv");
    std::fs::write(&p, "1\n".repeat(1000)).unwrap();
    let r = import_with(&ctx, &p, &opts(false), &|_, _| false, Some(20));
    assert_eq!(r.err().unwrap().kind(), io::ErrorKind::Interrupted);
}

/// 区切り文字 US（0x1F）・レコードの終わり RS（0x1E）の ASCII 区切りのデータ（値の改行はただの文字）。
fn random_ascii_delimited(rng: &mut Rng, records: usize) -> Vec<u8> {
    let mut out = Vec::new();
    for r in 0..records {
        let cols = 3 + rng.below(3) as usize;
        for c in 0..cols {
            if c > 0 {
                out.push(0x1F);
            }
            match rng.below(6) {
                0 => out.extend(format!("{}", rng.below(100_000)).bytes()),
                1 => out.extend(format!("\"x{r}\x1e\"\"{c}\"\"\x1fy\"").bytes()),
                2 => out.extend("改行\r\nと\nLF".bytes()),
                3 => {}
                4 => out.extend(format!("テキスト{}", rng.below(10)).bytes()),
                _ => out.extend(format!("{}.5", rng.below(1000)).bytes()),
            }
        }
        out.push(0x1E);
    }
    out
}

#[test]
fn ascii_delimited_detected_and_parallel() {
    let ctx = Context::for_tests();
    let dir = tempfile::tempdir().unwrap();
    for seed in 1..5u64 {
        let mut rng = Rng(seed * 104_729);
        let data = random_ascii_delimited(&mut rng, 300);
        let p = dir.path().join(format!("a{seed}.dat"));
        std::fs::write(&p, &data).unwrap();
        // US・RS を推定する
        let pv = preview(&p).unwrap();
        assert_eq!(pv.options.dialect.delimiter(), b"\x1f", "seed {seed}");
        assert_eq!(pv.options.record_end, RecordEnd::Custom(vec![0x1E]));
        let mut o = pv.options.clone();
        o.header = false;
        o.types = vec![ColType::Text; 6];
        let seq = import_with(&ctx, &p, &o, OK, Some(usize::MAX / 2)).unwrap();
        assert_eq!(seq.table.rows, 300, "seed {seed}");
        let g = grid(&ctx, &seq);
        // 値の中の改行・引用符の中の RS・US はただの文字
        assert!(g.iter().flatten().any(|v| *v == t("改行\r\nと\nLF")));
        assert!(
            g.iter()
                .flatten()
                .any(|v| matches!(v, Value::Text(s) if s.contains('\x1e') && s.contains('\x1f')))
        );
        // 区画の境目が引用符の中に入る小さな区画でも同じ
        for section in [13, 64, 501] {
            let par = import_with(&ctx, &p, &o, OK, Some(section)).unwrap();
            assert_eq!(grid(&ctx, &par), g, "seed {seed} section {section}");
        }
        // 書き出して読み直すと同じ（RS・US を含む値は引用符で囲む）
        let out = dir.path().join(format!("o{seed}.dat"));
        let eo = ExportOptions {
            dialect: o.dialect,
            record_end: o.record_end.clone(),
            formatted: false,
            ..ExportOptions::default()
        };
        export(&ctx, &seq, DateSystem::D1900, &out, &eo, None, OK).unwrap();
        let written = std::fs::read(&out).unwrap();
        assert!(!written.contains(&b'\n') || written.windows(2).any(|w| w == b"\r\n"));
        let back = import(&ctx, &out, &o, OK).unwrap();
        assert_eq!(grid(&ctx, &back), g, "seed {seed}");
    }
}

#[test]
fn tsv_cr_only_and_custom_separators() {
    let ctx = Context::for_tests();
    let dir = tempfile::tempdir().unwrap();
    // TSV（拡張子と中身から）
    let p = dir.path().join("t.tsv");
    std::fs::write(&p, "名前\t値\na, b\t1\n\"タ\tブ\"\t2\n").unwrap();
    let pv = preview(&p).unwrap();
    assert_eq!(pv.options.dialect, Dialect::tsv());
    assert_eq!(pv.options.record_end, RecordEnd::Newline);
    let s = import(&ctx, &p, &pv.options, OK).unwrap();
    assert_eq!(grid(&ctx, &s)[2], vec![t("タ\tブ"), n(2.0)]);
    let out = dir.path().join("o.tsv");
    let eo = ExportOptions {
        dialect: Dialect::tsv(),
        crlf: false,
        ..ExportOptions::default()
    };
    export(&ctx, &s, DateSystem::D1900, &out, &eo, None, OK).unwrap();
    assert_eq!(
        std::fs::read_to_string(&out).unwrap(),
        "名前\t値\na, b\t1\n\"タ\tブ\"\t2\n"
    );
    // CR だけで終わるレコード（値の LF はただの文字）
    let p = dir.path().join("mac.txt");
    std::fs::write(&p, "a;b\r1;x\r2;z\r").unwrap();
    let pv = preview(&p).unwrap();
    assert_eq!(pv.options.record_end, RecordEnd::Custom(vec![b'\r']));
    assert_eq!(pv.options.dialect.delimiter(), b";");
    // 指定すれば、値の LF はただの文字
    std::fs::write(&p, "a;b\r1;x\ny\r2;z\r").unwrap();
    let pv = preview_as(
        &p,
        Some(pv.options.dialect),
        Some(pv.options.record_end),
        None,
    )
    .unwrap();
    let s = import(&ctx, &p, &pv.options, OK).unwrap();
    assert_eq!(
        grid(&ctx, &s),
        vec![
            vec![t("a"), t("b")],
            vec![n(1.0), t("x\ny")],
            vec![n(2.0), t("z")]
        ]
    );
    // 指定した区切り（複数バイト・NUL）で書いて、指定して読む
    let d = Dialect::with_any_delimiter(b"\x00|", Some(b'"')).unwrap();
    let end = RecordEnd::Custom(b"\x1d\n".to_vec());
    let out = dir.path().join("custom.dat");
    let eo = ExportOptions {
        dialect: d,
        record_end: end.clone(),
        formatted: false,
        ..ExportOptions::default()
    };
    export(&ctx, &s, DateSystem::D1900, &out, &eo, None, OK).unwrap();
    assert_eq!(
        std::fs::read(&out).unwrap(),
        b"a\x00|b\x1d\n1\x00|\"x\ny\"\x1d\n2\x00|z\x1d\n"
    );
    let pv = preview_as(&out, Some(d), Some(end), Some(Encoding::Utf8)).unwrap();
    let back = import(&ctx, &out, &pv.options, OK).unwrap();
    assert_eq!(grid(&ctx, &back), grid(&ctx, &s));
    // LF を区切り文字に（レコードの終わりが改行以外なら使える）
    let d = Dialect::with_any_delimiter(b"\n", None).unwrap();
    let end = RecordEnd::Custom(vec![0x1E]);
    let p = dir.path().join("lf.dat");
    std::fs::write(&p, b"a\nb\x1ec\nd\x1e").unwrap();
    let mut o = opts(false);
    o.dialect = d;
    o.record_end = end;
    let s = import(&ctx, &p, &o, OK).unwrap();
    assert_eq!(
        grid(&ctx, &s),
        vec![vec![t("a"), t("b")], vec![t("c"), t("d")]]
    );
}

#[test]
fn custom_terminator_with_shift_jis() {
    let ctx = Context::for_tests();
    let dir = tempfile::tempdir().unwrap();
    let text = "名前\x1f数量\x1eソフト表示\x1f1\x1e\"能力\x1e予定\"\x1f2\x1e";
    let p = dir.path().join("sj.dat");
    let bytes =
        yy_encoding::encode_all(Encoding::Cp932, text.as_bytes(), EscapeMode::Literal).unwrap();
    std::fs::write(&p, &bytes).unwrap();
    let d = Dialect::new(b"\x1f", Some(b'"')).unwrap();
    let pv = preview_as(
        &p,
        Some(d),
        Some(RecordEnd::Custom(vec![0x1E])),
        Some(Encoding::Cp932),
    )
    .unwrap();
    assert!(pv.options.header);
    for section in [None, Some(5)] {
        let s = import_with(&ctx, &p, &pv.options, OK, section).unwrap();
        let g = grid(&ctx, &s);
        assert_eq!(g[1], vec![t("ソフト表示"), n(1.0)]);
        assert_eq!(g[2], vec![t("能力\x1e予定"), n(2.0)]);
    }
}

#[test]
fn separator_combinations_are_checked() {
    let lf = Dialect::with_any_delimiter(b"\n", Some(b'"')).unwrap();
    assert!(check_separators(&lf, &RecordEnd::Newline).is_err());
    assert!(check_separators(&lf, &RecordEnd::Custom(vec![0x1E])).is_ok());
    let csv = Dialect::csv();
    assert!(check_separators(&csv, &RecordEnd::Custom(vec![])).is_err());
    assert!(check_separators(&csv, &RecordEnd::Custom(b",,".to_vec())).is_err());
    assert!(check_separators(&csv, &RecordEnd::Custom(b"\"".to_vec())).is_err());
    assert!(check_separators(&csv, &RecordEnd::Custom("。".as_bytes().to_vec())).is_err());
    assert!(check_separators(&csv, &RecordEnd::Custom(b"\r".to_vec())).is_ok());
    assert_eq!(RecordEnd::Custom(vec![0x1E]).label(), "<RS>");
}
