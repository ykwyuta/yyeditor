//! 「デコード → エンコード」でバイト列が変わらないことの検証（08 章 2.1）。

use proptest::prelude::*;
use yy_encoding::{Encoding, EscapeMode, decode_all, detect, encode_all, escape_char};

const DBCS: [Encoding; 5] = [
    Encoding::ShiftJis,
    Encoding::Cp932,
    Encoding::ShiftJis2004,
    Encoding::EucJp,
    Encoding::EucJis2004,
];

/// 往復でバイト列が保たれることを保証する文字コード（ISO-2022-JP 以外）。
fn lossless() -> Vec<Encoding> {
    let mut v = vec![
        Encoding::Utf16Le,
        Encoding::Utf16Be,
        Encoding::Utf32Le,
        Encoding::Utf32Be,
    ];
    v.extend(DBCS);
    v.push(Encoding::from_name("windows-1252").unwrap());
    v.push(Encoding::from_name("koi8-r").unwrap());
    v
}

fn roundtrip(enc: Encoding, bytes: &[u8]) -> (Vec<u8>, yy_encoding::DecodeStats) {
    let (text, stats) = decode_all(enc, bytes, true);
    let out = encode_all(enc, &text, EscapeMode::Restore)
        .unwrap_or_else(|bad| panic!("{enc}: unmappable {bad:?} in {bytes:02X?}"));
    (out, stats)
}

/// すべての定義済み符号（1〜3 バイト）が往復する。重複符号の数も確認する。
#[test]
fn every_code_roundtrips() {
    for enc in DBCS {
        let mut codes: Vec<Vec<u8>> = (0..=0xFFu8).map(|b| vec![b]).collect();
        for l in 0x80..=0xFFu8 {
            for t in 0x40..=0xFFu8 {
                codes.push(vec![l, t]);
            }
        }
        if matches!(enc, Encoding::EucJp | Encoding::EucJis2004) {
            for l in 0xA1..=0xFEu8 {
                for t in 0xA1..=0xFEu8 {
                    codes.push(vec![0x8F, l, t]);
                }
            }
        }
        let (mut defined, mut noncanonical) = (0, 0);
        for code in codes {
            let (text, stats) = decode_all(enc, &code, true);
            let s = String::from_utf8(text.clone()).unwrap();
            let valid = stats.invalid == 0;
            if valid {
                defined += 1;
            }
            let out = encode_all(enc, &text, EscapeMode::Restore).unwrap();
            if stats.noncanonical > 0 {
                noncanonical += 1;
                // 優先される符号に正規化され、それを読むと同じ文字になる
                assert_ne!(out, code);
                assert_eq!(decode_all(enc, &out, true).0, text, "{enc} {code:02X?}");
            } else {
                assert_eq!(out, code, "{enc}: {code:02X?} -> {s:?}");
            }
        }
        eprintln!("{enc}: {defined} defined, {noncanonical} non-canonical");
        let expect_noncanonical = match enc {
            Encoding::Cp932 | Encoding::EucJp => 1..1000,
            _ => 0..1,
        };
        assert!(
            expect_noncanonical.contains(&noncanonical),
            "{enc}: {noncanonical} non-canonical"
        );
        assert!(defined > 7000, "{enc}: {defined}");
    }
}

#[test]
fn jis_and_microsoft_mappings() {
    let wave = "\u{301C}".as_bytes();
    let tilde = "\u{FF5E}".as_bytes();
    // 0x8160 は JIS 準拠では波ダッシュ、CP932 では全角チルダ
    assert_eq!(decode_all(Encoding::ShiftJis, b"\x81\x60", true).0, wave);
    assert_eq!(decode_all(Encoding::Cp932, b"\x81\x60", true).0, tilde);
    // 保存時はどちらの文字も 0x8160 にする
    for enc in [Encoding::ShiftJis, Encoding::Cp932] {
        for t in [wave, tilde] {
            assert_eq!(
                encode_all(enc, t, EscapeMode::Restore).unwrap(),
                b"\x81\x60"
            );
        }
    }
    // CP932 の拡張文字（NEC 特殊文字 ①）は JIS 準拠の Shift_JIS では不正バイト扱い
    let circled = "\u{2460}".as_bytes();
    assert_eq!(decode_all(Encoding::Cp932, b"\x87\x40", true).0, circled);
    let (t, s) = decode_all(Encoding::ShiftJis, b"\x87\x40", true);
    assert_eq!(s.invalid, 2);
    assert_eq!(
        t,
        format!("{}{}", escape_char(0x87), escape_char(0x40)).as_bytes()
    );
    assert!(encode_all(Encoding::ShiftJis, circled, EscapeMode::Restore).is_err());
    // NEC 選定 IBM 拡張（0xED40）は IBM 拡張（0xFA5C）に正規化される
    let (t, s) = decode_all(Encoding::Cp932, b"\xED\x40", true);
    assert_eq!(s.noncanonical, 1);
    assert_eq!(
        encode_all(Encoding::Cp932, &t, EscapeMode::Restore).unwrap(),
        b"\xFA\x5C"
    );
}

#[test]
fn jis_x_0213_combining_pairs() {
    // 0x82F5 = か + 半濁点（2 文字）
    let text = "\u{304B}\u{309A}";
    assert_eq!(
        decode_all(Encoding::ShiftJis2004, b"\x82\xF5", true).0,
        text.as_bytes()
    );
    assert_eq!(
        encode_all(Encoding::ShiftJis2004, text.as_bytes(), EscapeMode::Restore).unwrap(),
        b"\x82\xF5"
    );
    // か だけなら通常の符号
    assert_eq!(
        encode_all(
            Encoding::ShiftJis2004,
            "\u{304B}a".as_bytes(),
            EscapeMode::Restore
        )
        .unwrap(),
        b"\x82\xA9a"
    );
    // EUC-JIS-2004 では A4F7
    assert_eq!(
        encode_all(Encoding::EucJis2004, text.as_bytes(), EscapeMode::Restore).unwrap(),
        b"\xA4\xF7"
    );
    // 区切り位置が組の間でも同じ結果
    let mut e = Encoding::ShiftJis2004.new_encoder(EscapeMode::Restore);
    let mut out = Vec::new();
    let b = text.as_bytes();
    e.encode(&b[..3], &mut out, false, &mut |_| panic!());
    e.encode(&b[3..], &mut out, true, &mut |_| panic!());
    assert_eq!(out, b"\x82\xF5");
    // 第 3・第 4 水準（面 2、U+20B9F 𠮟）
    let (t, s) = decode_all(Encoding::EucJis2004, b"\x8F\xA1\xA1", true);
    assert_eq!(s.invalid, 0);
    assert_eq!(
        encode_all(Encoding::EucJis2004, &t, EscapeMode::Restore).unwrap(),
        b"\x8F\xA1\xA1"
    );
    assert_eq!(
        encode_all(Encoding::ShiftJis2004, &t, EscapeMode::Restore).unwrap(),
        b"\xF0\x40"
    );
}

#[test]
fn euc_jp_supplementary_kanji() {
    // JIS X 0212（3 バイト）も保存できる
    let (t, s) = decode_all(Encoding::EucJp, b"\x8F\xB0\xA1", true);
    assert_eq!(s.invalid, 0);
    assert_eq!(String::from_utf8(t.clone()).unwrap(), "\u{4E02}");
    assert_eq!(
        encode_all(Encoding::EucJp, &t, EscapeMode::Restore).unwrap(),
        b"\x8F\xB0\xA1"
    );
}

#[test]
fn utf16_keeps_unpaired_surrogates() {
    // "a" + 不対の上位サロゲート + "b" + 奇数バイト
    let bytes = b"a\0\x00\xD8b\0\x01";
    let (t, s) = decode_all(Encoding::Utf16Le, bytes, true);
    assert_eq!(s.invalid, 3);
    assert_eq!(
        String::from_utf8(t.clone()).unwrap(),
        format!(
            "a{}{}b{}",
            escape_char(0),
            escape_char(0xD8),
            escape_char(1)
        )
    );
    assert_eq!(
        encode_all(Encoding::Utf16Le, &t, EscapeMode::Restore).unwrap(),
        bytes
    );
    // 別の文字コードで保存する場合はエスケープ文字を変換できない
    assert_eq!(
        encode_all(Encoding::Utf8, &t, EscapeMode::Reject).unwrap_err(),
        vec![1..5, 5..9, 10..14]
    );
}

#[test]
fn literal_escape_characters_are_counted() {
    let text = format!("x{}", escape_char(0x41));
    let bytes = encode_all(Encoding::Utf16Le, text.as_bytes(), EscapeMode::Literal).unwrap();
    let (t, s) = decode_all(Encoding::Utf16Le, &bytes, true);
    assert_eq!(s.literal_escapes, 1);
    assert_eq!(t, text.as_bytes());
}

#[test]
fn reports_unmappable_ranges() {
    let text = "aあb😀".as_bytes();
    let bad = encode_all(
        Encoding::from_name("windows-1252").unwrap(),
        text,
        EscapeMode::Reject,
    )
    .unwrap_err();
    assert_eq!(bad, vec![1..4, 5..9]);
    let bad = encode_all(Encoding::Cp932, text, EscapeMode::Reject).unwrap_err();
    assert_eq!(bad, vec![5..9]);
    // UTF-8 由来の不正なバイトも UTF-8 以外には変換できない
    let bad = encode_all(Encoding::Cp932, b"a\xFFb", EscapeMode::Literal).unwrap_err();
    assert_eq!(bad, vec![1..2]);
    // UTF-8 へはそのまま
    assert_eq!(
        encode_all(Encoding::Utf8, b"a\xFFb", EscapeMode::Literal).unwrap(),
        b"a\xFFb"
    );
}

#[test]
fn iso_2022_jp_canonical_roundtrip() {
    let bytes = b"Hello \x1B$B$3$s$K$A$O\x1B(B world\r\n\x1B$B4A;z\x1B(B\r\n";
    let (t, s) = decode_all(Encoding::Iso2022Jp, bytes, true);
    assert_eq!(s.invalid, 0);
    assert_eq!(
        String::from_utf8(t.clone()).unwrap(),
        "Hello こんにちは world\r\n漢字\r\n"
    );
    assert_eq!(
        encode_all(Encoding::Iso2022Jp, &t, EscapeMode::Restore).unwrap(),
        bytes
    );
}

#[test]
fn detects_common_encodings() {
    let jp = "日本語のテキストです。ひらがな、カタカナ、漢字を含みます。\r\n";
    let enc = |e: Encoding| encode_all(e, jp.repeat(5).as_bytes(), EscapeMode::Reject).unwrap();
    for (e, expect) in [
        (Encoding::Cp932, Encoding::Cp932),
        (Encoding::EucJp, Encoding::EucJp),
        (Encoding::Iso2022Jp, Encoding::Iso2022Jp),
        (Encoding::Utf8, Encoding::Utf8),
        (Encoding::Utf16Le, Encoding::Utf16Le),
        (Encoding::Utf16Be, Encoding::Utf16Be),
    ] {
        let d = detect(&enc(e), true);
        assert_eq!(d.encoding, expect, "{e}");
        assert_eq!(d.bom_len, 0);
    }
    // 途中で切れたサンプルでも判別できる
    let u = jp.repeat(5);
    let d = detect(&u.as_bytes()[..u.len() - 1], false);
    assert_eq!(d.encoding, Encoding::Utf8);
    // BOM
    let d = detect(b"\xFF\xFEa\0", true);
    assert_eq!((d.encoding, d.bom_len), (Encoding::Utf16Le, 2));
    let d = detect(b"\xEF\xBB\xBFabc", true);
    assert_eq!((d.encoding, d.bom_len), (Encoding::Utf8, 3));
    assert_eq!(detect(b"plain ascii", true).encoding, Encoding::Utf8);
    assert_eq!(detect(b"", true).encoding, Encoding::Utf8);
    // 半角カナだけの行を含む CP932
    let kana = encode_all(
        Encoding::Cp932,
        "ｱｲｳｴｵ 全角もある文章です。\n".as_bytes(),
        EscapeMode::Reject,
    )
    .unwrap();
    assert_eq!(detect(&kana, true).encoding, Encoding::Cp932);
    // 不正なバイトを少し含む短い CP932
    let mut broken = encode_all(
        Encoding::Cp932,
        "日本語のテキスト。\r\n".as_bytes(),
        EscapeMode::Reject,
    )
    .unwrap();
    broken.extend_from_slice(b"\xFF\x80 end\r\n");
    assert_eq!(detect(&broken, true).encoding, Encoding::Cp932);
    // 欧文の windows-1252 は日本語にしない
    let latin = b"Caf\xE9 cr\xE8me br\xFBl\xE9e, na\xEFve fa\xE7ade.\r\n".repeat(4);
    assert_eq!(
        detect(&latin, true).encoding,
        Encoding::from_name("windows-1252").unwrap()
    );
}

#[test]
fn names_roundtrip() {
    for e in Encoding::all() {
        assert_eq!(Encoding::from_name(e.name()), Some(e), "{e}");
    }
    assert_eq!(Encoding::from_name("sjis"), Some(Encoding::ShiftJis));
    assert_eq!(Encoding::from_name("Windows-31J"), Some(Encoding::Cp932));
    assert_eq!(
        Encoding::from_name("shift_jis-2004"),
        Some(Encoding::ShiftJis2004)
    );
    assert_eq!(Encoding::from_name("euc_jp"), Some(Encoding::EucJp));
}

/// 日本語の文字コードで出やすいバイトに偏らせたランダムなバイト列。
fn bytes_strategy() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(
        prop_oneof![
            3 => 0x20u8..0x7F,
            1 => Just(b'\n'),
            4 => 0x81u8..0xA0,
            4 => 0xA1u8..0xFF,
            2 => 0xE0u8..=0xFF,
            1 => 0x00u8..=0xFF,
            1 => Just(0x8Fu8),
            1 => Just(0x8Eu8),
        ],
        0..200,
    )
}

fn decode_chunked(enc: Encoding, bytes: &[u8], cuts: &[usize]) -> Vec<u8> {
    let mut d = enc.new_decoder(true);
    let mut out = Vec::new();
    let mut prev = 0;
    for &c in cuts {
        let c = c.clamp(prev, bytes.len());
        d.decode(&bytes[prev..c], &mut out, false);
        prev = c;
    }
    d.decode(&bytes[prev..], &mut out, true);
    out
}

fn encode_chunked(enc: Encoding, text: &[u8], cuts: &[usize]) -> Vec<u8> {
    let mut e = enc.new_encoder(EscapeMode::Restore);
    let mut out = Vec::new();
    let mut prev = 0;
    for &c in cuts {
        let c = c.clamp(prev, text.len());
        e.encode(&text[prev..c], &mut out, false, &mut |r| panic!("{r:?}"));
        prev = c;
    }
    e.encode(&text[prev..], &mut out, true, &mut |r| panic!("{r:?}"));
    out
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(400))]

    /// 任意のバイト列が往復する（重複符号・ファイル内のエスケープ文字を含む場合を除く）。
    #[test]
    fn random_bytes_roundtrip(bytes in bytes_strategy()) {
        for enc in lossless() {
            let (out, stats) = roundtrip(enc, &bytes);
            if stats.noncanonical == 0 && stats.literal_escapes == 0 {
                prop_assert_eq!(&out, &bytes, "{}", enc);
            }
        }
    }

    /// LF の直後で区切って別々のデコーダで読んでも結果は同じ（並列デコードの前提）。
    #[test]
    fn independent_decoding_after_lf(bytes in bytes_strategy()) {
        for enc in Encoding::all().into_iter().filter(|e| e.splits_at_lf()) {
            let whole = decode_all(enc, &bytes, true);
            let mut out = Vec::new();
            let mut stats = yy_encoding::DecodeStats::default();
            for seg in bytes.split_inclusive(|&b| b == b'\n') {
                let (t, s) = decode_all(enc, seg, true);
                out.extend(t);
                stats.invalid += s.invalid;
                stats.noncanonical += s.noncanonical;
                stats.literal_escapes += s.literal_escapes;
            }
            prop_assert_eq!(&out, &whole.0, "{}", enc);
            prop_assert_eq!(stats, whole.1, "{}", enc);
        }
    }

    /// 入力をどこで区切ってもデコード・エンコードの結果は同じ。
    #[test]
    fn chunking_does_not_matter(
        bytes in bytes_strategy(),
        mut cuts in prop::collection::vec(0usize..200, 0..6),
    ) {
        cuts.sort();
        let mut all = lossless();
        all.push(Encoding::Iso2022Jp);
        for enc in all {
            let whole = decode_all(enc, &bytes, true).0;
            prop_assert_eq!(&decode_chunked(enc, &bytes, &cuts), &whole, "{}", enc);
            if let Ok(encoded) = encode_all(enc, &whole, EscapeMode::Restore) {
                prop_assert_eq!(&encode_chunked(enc, &whole, &cuts), &encoded, "{}", enc);
            }
        }
    }
}

/// 組になりうる文字が連続しても、どこで区切っても同じ結果になる。
#[test]
fn combining_chains_at_every_cut() {
    let alphabet = ["\u{304B}", "\u{304B}\u{309A}", "\u{02E9}", "\u{02E5}", "a"];
    let mut seed = 1u64;
    for _ in 0..300 {
        let mut text = String::new();
        for _ in 0..8 {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            text.push_str(alphabet[(seed >> 33) as usize % alphabet.len()]);
        }
        for enc in [Encoding::ShiftJis2004, Encoding::EucJis2004] {
            let whole = encode_all(enc, text.as_bytes(), EscapeMode::Restore).unwrap();
            for cut in 0..=text.len() {
                for cut2 in cut..=text.len() {
                    assert_eq!(
                        encode_chunked(enc, text.as_bytes(), &[cut, cut2]),
                        whole,
                        "{enc} {text:?} {cut} {cut2}"
                    );
                }
            }
        }
    }
}
