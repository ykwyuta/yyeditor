//! 外部の対応表（`.map`）: ベンダー漢字コード・外字（03 章 2.3）。

use yy_encoding::{
    Encoding, EscapeMode, Records, decode_all, encode_all, parse_mapping, register_mapping,
};

fn decode(e: Encoding, bytes: &[u8]) -> String {
    let (t, s) = decode_all(e, bytes, true);
    assert_eq!(s.invalid, 0, "{e:?} {bytes:02X?}");
    String::from_utf8(t).unwrap()
}

fn encode(e: Encoding, text: &str) -> Vec<u8> {
    encode_all(e, text.as_bytes(), EscapeMode::Restore).unwrap()
}

#[test]
fn ebcdic_overlay_adds_and_replaces_codes() {
    let m = parse_mapping(
        "gaiji930",
        "# IBM-930 に外字を加える\n\
         @base IBM-930\n\
         0x6941\tU+20B9F\n\
         0x4F58\tU+2000B   # 漢 の符号を別の文字にする\n",
    )
    .unwrap();
    let e = register_mapping(m).unwrap();
    assert_eq!(e.name(), "gaiji930");
    assert_eq!(e.records(), Some(Records::Nl));
    assert_eq!(
        decode(e, b"\xC1\x0E\x69\x41\x4F\x58\x0F"),
        "A\u{20B9F}\u{2000B}"
    );
    assert_eq!(
        encode(e, "A\u{20B9F}\u{2000B}"),
        b"\xC1\x0E\x69\x41\x4F\x58\x0F"
    );
    // 置き換えた元の文字は変換できない。他の文字は土台のまま
    assert!(encode_all(e, "漢".as_bytes(), EscapeMode::Restore).is_err());
    assert_eq!(encode(e, "字"), b"\x0E\x48\xF2\x0F");
    // 名前（レコードの区切り方つき）で探せる
    let fixed = Encoding::from_name("GAIJI930/fixed:80").unwrap();
    assert_eq!(fixed.records(), Some(Records::Fixed(80)));
    assert!(fixed.same_charset(&e));
    assert_eq!(Encoding::from_name(&fixed.spec()), Some(fixed));
    assert!(Encoding::all().contains(&e));
}

#[test]
fn vendor_code_with_own_shift_bytes() {
    // JEF のように独自のシフト（0x28 / 0x29）で 2 バイト部に入る文字コード
    let m = parse_mapping(
        "x",
        "@name TESTJEF\n\
         @shift 0x28 0x29\n\
         40\tU+0020\n\
         C1\tU+0041\n\
         C2\tU+0042\n\
         15\tU+000A\n\
         B0A1\tU+4E9C\n\
         B0A2\tU+5516\n",
    )
    .unwrap();
    let e = register_mapping(m).unwrap();
    let bytes = b"\xC1\x28\xB0\xA1\xB0\xA2\x29\x40\xC2\x15";
    assert_eq!(decode(e, bytes), "A亜唖 B\n");
    assert_eq!(encode(e, "A亜唖 B\n"), bytes);
    // 表にない文字は変換できない
    assert!(encode_all(e, "C".as_bytes(), EscapeMode::Restore).is_err());
}

#[test]
fn cp932_gaiji_overlay() {
    let m = parse_mapping(
        "gaiji932",
        "@base CP932\n0xF040\tU+20B9F\n0xF041\tU+2000B\n",
    )
    .unwrap();
    let e = register_mapping(m).unwrap();
    assert_eq!(e.records(), None);
    assert!(e.splits_at_lf());
    assert_eq!(
        decode(e, b"a\xF0\x40\xF0\x41\x82\xA0"),
        "a\u{20B9F}\u{2000B}あ"
    );
    assert_eq!(
        encode(e, "a\u{20B9F}\u{2000B}あ"),
        b"a\xF0\x40\xF0\x41\x82\xA0"
    );
    // 標準の CP932 では私用領域の文字
    assert_eq!(decode(Encoding::Cp932, b"\xF0\x40"), "\u{E000}");
}

#[test]
fn reports_errors_with_line_numbers() {
    for (text, expected) in [
        ("@base UTF-8\n", "1 行目"),
        ("@base CP932\n\n0xF040\tU+ZZZZ\n", "3 行目"),
        ("0x123\tU+0041\n", "1 行目"),
        ("C1\n", "1 行目"),
        ("@shift 0x0E\n", "1 行目"),
        ("@frobnicate\n", "1 行目"),
        ("@base CP932\n@shift 0x0E 0x0F\n", "@shift"),
        ("@name a/b\n", "@name"),
    ] {
        let err = parse_mapping("t", text).unwrap_err();
        assert!(err.contains(expected), "{text:?}: {err}");
    }
    // 組み込みの文字コードと同じ名前は登録できない
    let m = parse_mapping("t", "@name CP932\n@base CP932\n").unwrap();
    assert!(register_mapping(m).is_err());
}
