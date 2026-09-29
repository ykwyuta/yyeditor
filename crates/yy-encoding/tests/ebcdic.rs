//! EBCDIC（03 章 1.3）: IBM 930 / 939 などのサンプルデータでの往復一致（08 章 M7）。

use proptest::prelude::*;
use yy_encoding::{
    Ccsid, Encoding, EscapeMode, Records, decode_all, detect_ebcdic, encode_all, escape_char,
};

fn enc(c: Ccsid, r: Records) -> Encoding {
    Encoding::Ebcdic(c, r)
}

fn decode_str(e: Encoding, bytes: &[u8]) -> String {
    let (t, s) = decode_all(e, bytes, true);
    assert_eq!(s.invalid, 0, "{e:?} {bytes:02X?}");
    String::from_utf8(t).unwrap()
}

fn encode(e: Encoding, text: &str) -> Vec<u8> {
    encode_all(e, text.as_bytes(), EscapeMode::Restore).unwrap()
}

/// デコード → エンコードでバイト列が戻る（正規化される場合はそれを読むと同じ文字）。
fn check_roundtrip(e: Encoding, bytes: &[u8]) {
    let (text, stats) = decode_all(e, bytes, true);
    let out = encode_all(e, &text, EscapeMode::Restore)
        .unwrap_or_else(|bad| panic!("{e:?}: unmappable {bad:?} in {bytes:02X?}"));
    if stats.noncanonical == 0 {
        assert_eq!(out, bytes, "{e:?}");
    } else {
        assert_eq!(decode_all(e, &out, true).0, text, "{e:?} {bytes:02X?}");
    }
}

#[test]
fn single_byte_samples() {
    let e = enc(Ccsid::Ibm037, Records::Nl);
    assert_eq!(
        encode(e, "Hello, World!"),
        b"\xC8\x85\x93\x93\x96\x6B\x40\xE6\x96\x99\x93\x84\x5A"
    );
    assert_eq!(decode_str(e, b"\xC8\x85\x93\x93\x96"), "Hello");
    // 930 の 1 バイト部は カタカナ、939 は英小文字
    assert_eq!(
        decode_str(enc(Ccsid::Ibm930, Records::Nl), b"\x81"),
        "\u{FF71}"
    );
    assert_eq!(decode_str(enc(Ccsid::Ibm939, Records::Nl), b"\x81"), "a");
    assert_eq!(decode_str(enc(Ccsid::Ibm930, Records::Nl), b"\x62"), "a");
    // ICU の Unicode → 符号のみの対応（全角英数 → 半角など）は使わず、変換できない文字にする
    assert!(encode_all(e, "Ｗ".as_bytes(), EscapeMode::Restore).is_err());
}

#[test]
fn mixed_samples() {
    let e = enc(Ccsid::Ibm939, Records::Nl);
    // "AB" SO 漢字 SI "C"
    let bytes = b"\xC1\xC2\x0E\x4F\x58\x48\xF2\x0F\xC3";
    assert_eq!(decode_str(e, bytes), "AB漢字C");
    assert_eq!(encode(e, "AB漢字C"), bytes);
    // 行末・入力の終わりの前には SI を出す
    assert_eq!(encode(e, "漢\n字"), b"\x0E\x4F\x58\x0F\x15\x0E\x48\xF2\x0F");
    // 全角スペース
    assert_eq!(encode(e, "\u{3000}"), b"\x0E\x40\x40\x0F");
    // 1390 は € を 1 バイトで持つ（2 バイトの 0x42E1 は読むだけ）
    let e = enc(Ccsid::Ibm1390, Records::Nl);
    assert_eq!(encode(e, "€"), b"\xE1");
    let (t, s) = decode_all(e, b"\x0E\x42\xE1\x0F", true);
    assert_eq!(String::from_utf8(t).unwrap(), "€");
    assert_eq!(s.noncanonical, 1);
}

#[test]
fn record_formats() {
    let text = "AB\nC";
    let nl = enc(Ccsid::Ibm037, Records::Nl);
    let lf = enc(Ccsid::Ibm037, Records::Lf);
    assert_eq!(encode(nl, text), b"\xC1\xC2\x15\xC3");
    assert_eq!(encode(lf, text), b"\xC1\xC2\x25\xC3");
    assert_eq!(decode_str(nl, b"\xC1\xC2\x15\xC3"), text);
    assert_eq!(decode_str(lf, b"\xC1\xC2\x25\xC3"), text);
    // もう一方の改行は U+0085（NEL）
    assert_eq!(decode_str(nl, b"\x25"), "\u{85}");
    assert_eq!(decode_str(lf, b"\x15"), "\u{85}");

    // 固定長: 4 バイトごとに 1 行
    let fixed = enc(Ccsid::Ibm939, Records::Fixed(4));
    assert_eq!(
        decode_str(fixed, b"\xC1\xC2\x40\x40\x0E\x4F\x58\x0F\xC3"),
        "AB  \n漢\nC"
    );
    // 短い行は空白で埋める。最後の改行のない行はそのまま
    assert_eq!(
        encode(fixed, "AB\n漢\nC"),
        b"\xC1\xC2\x40\x40\x0E\x4F\x58\x0F\xC3"
    );
    // ちょうど割り切れるファイルは最後に改行がつき、そのまま書き戻せる
    assert_eq!(decode_str(fixed, b"\xC1\xC2\xC3\xC4"), "ABCD\n");
    assert_eq!(encode(fixed, "ABCD\n"), b"\xC1\xC2\xC3\xC4");
    // レコード長を超える行は保存できない（超えた文字を報告する）
    let bad = encode_all(fixed, "ABCDE\n".as_bytes(), EscapeMode::Restore).unwrap_err();
    assert_eq!(bad, vec![4..5]);
    let bad = encode_all(fixed, "AB漢\n".as_bytes(), EscapeMode::Restore).unwrap_err();
    assert_eq!(bad, vec![2..5]);
    // レコードがちょうどいっぱいなら SI を出さない（次のレコードは 1 バイト部から始まる）
    assert_eq!(encode(fixed, "A漢\nB"), b"\xC1\x0E\x4F\x58\xC2");
    assert_eq!(decode_str(fixed, b"\xC1\x0E\x4F\x58\xC2"), "A漢\nB");
    // 固定長では 0x25 は改行ではない
    let (t, s) = decode_all(fixed, b"\xC1\x25", true);
    assert_eq!(s.invalid, 1);
    assert_eq!(
        String::from_utf8(t.clone()).unwrap(),
        format!("A{}", escape_char(0x25))
    );
    assert_eq!(
        encode_all(fixed, &t, EscapeMode::Restore).unwrap(),
        b"\xC1\x25"
    );
}

#[test]
fn irregular_shifts_roundtrip() {
    let e = enc(Ccsid::Ibm930, Records::Nl);
    for bytes in [
        &b"\x0E\x0F"[..],                // 空の 2 バイト部
        b"\xC1\x0E\x0F\xC2",             // 途中の空の 2 バイト部
        b"\x0E\x0F\x0E\x4F\x58\x0F",     // 空の後の 2 バイト部
        b"\x0F\xC1",                     // 1 バイト部での SI
        b"\x0E\x0E\x4F\x58\x0F",         // 重複した SO
        b"\x0E\x4F\x58\x0F\x0F",         // 重複した SI
        b"\x0E\xFF\xFF\x4F\x58\x0F",     // 未定義の 2 バイト符号
        b"\x0E\x4F\x58\xFF\xFF\x0F\xC1", // 2 バイト部の途中の未定義符号
        b"\x0E\x4F\x58\x15\x0F",         // 2 バイト部の途中の奇数バイト
    ] {
        check_roundtrip(e, bytes);
        let (t, s) = decode_all(e, bytes, true);
        let out = encode_all(e, &t, EscapeMode::Restore).unwrap();
        assert_eq!(out, bytes, "{bytes:02X?} (noncanonical {})", s.noncanonical);
    }
    // SO だけで終わるデータもそのまま
    check_roundtrip(e, b"\xC1\x0E");
    assert_eq!(decode_all(e, b"\xC1\x0E", true).1.noncanonical, 0);
    // 入力の終わりが 2 バイト部（SI がない）なら SI を補う（正規化）
    let (t, s) = decode_all(e, b"\x0E\x4F\x58", true);
    assert_eq!(s.noncanonical, 1);
    assert_eq!(
        encode_all(e, &t, EscapeMode::Restore).unwrap(),
        b"\x0E\x4F\x58\x0F"
    );
}

#[test]
fn every_code_roundtrips() {
    for c in Ccsid::ALL {
        let e = enc(c, Records::Lf);
        let mut defined = 0;
        for b in 0..=0xFFu8 {
            check_roundtrip(e, &[b]);
            if decode_all(e, &[b], true).1.invalid == 0 {
                defined += 1;
            }
        }
        if c.is_mixed() {
            for code in 0x4040..=0xFEFEu16 {
                let [x, y] = code.to_be_bytes();
                let bytes = [0x0E, x, y, 0x0F];
                check_roundtrip(e, &bytes);
                if decode_all(e, &bytes, true).1.invalid == 0 {
                    defined += 1;
                }
            }
            assert!(defined > 11000, "{c:?}: {defined}");
        } else {
            assert!(defined >= 190, "{c:?}: {defined}");
        }
    }
}

#[test]
fn names() {
    for (name, e) in [
        ("IBM-930", enc(Ccsid::Ibm930, Records::Nl)),
        ("ibm939", enc(Ccsid::Ibm939, Records::Nl)),
        ("CCSID 5026", enc(Ccsid::Ibm930, Records::Nl)),
        ("cp037/lf", enc(Ccsid::Ibm037, Records::Lf)),
        ("IBM-1390/fixed:80", enc(Ccsid::Ibm1390, Records::Fixed(80))),
    ] {
        assert_eq!(Encoding::from_name(name), Some(e), "{name}");
        assert_eq!(Encoding::from_name(&e.spec()), Some(e));
    }
    assert_eq!(Encoding::from_name("cp932"), Some(Encoding::Cp932));
    assert_eq!(Encoding::from_name("IBM-930/fixed:0"), None);
    assert!(Encoding::all().contains(&enc(Ccsid::Ibm939, Records::Nl)));
}

#[test]
fn detects_ebcdic_when_enabled() {
    let e939 = enc(Ccsid::Ibm939, Records::Nl);
    let text = "IDENTIFICATION DIVISION.\nPROGRAM-ID. SAMPLE.\n* 漢字のコメント\n".repeat(4);
    let bytes = encode(e939, &text);
    assert_eq!(detect_ebcdic(&bytes, bytes.len() as u64), Some(e939));
    // 改行がなければ固定長（ファイルの大きさを割り切る長さ）
    let fixed = enc(Ccsid::Ibm037, Records::Fixed(80));
    let rec = format!("{:<80}", "HELLO WORLD 0123456789");
    let bytes = encode(fixed, &format!("{rec}\n{rec}\n"));
    assert_eq!(bytes.len(), 160);
    assert_eq!(detect_ebcdic(&bytes, 160), Some(fixed));
    // 930 と 939 は英小文字の読めるほうを選ぶ
    let e930 = enc(Ccsid::Ibm930, Records::Nl);
    let bytes = encode(e930, &"DISPLAY 'hello, world'.\n* 漢字\n".repeat(4));
    assert_eq!(detect_ebcdic(&bytes, bytes.len() as u64), Some(e930));
    // ASCII・UTF-8 のテキストは EBCDIC としない
    assert_eq!(
        detect_ebcdic(b"Hello, world. This is ASCII text.\n", 34),
        None
    );
    assert_eq!(detect_ebcdic("日本語のテキストです。".as_bytes(), 33), None);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]

    /// 任意のバイト列が往復し、どこで区切ってデコードしても結果が同じ。
    #[test]
    fn random_bytes_roundtrip(
        bytes in proptest::collection::vec(
            prop_oneof![
                4 => any::<u8>(),
                2 => Just(0x0Eu8),
                2 => Just(0x0Fu8),
                2 => 0x40u8..0x70,
            ],
            0..80,
        ),
        split in 0usize..80,
        ci in 0usize..9,
        ri in 0usize..3,
    ) {
        let records = [Records::Nl, Records::Lf, Records::Fixed(7)][ri];
        let e = enc(Ccsid::ALL[ci], records);
        check_roundtrip(e, &bytes);
        let (whole, _) = decode_all(e, &bytes, true);
        let mut d = e.new_decoder(true);
        let mut out = Vec::new();
        let k = split.min(bytes.len());
        d.decode(&bytes[..k], &mut out, false);
        d.decode(&bytes[k..], &mut out, true);
        prop_assert_eq!(out, whole);
    }

    /// 文書（日本語混じり）→ エンコード → デコードで元に戻る。
    #[test]
    fn random_text_roundtrip(
        chars in proptest::collection::vec(
            prop_oneof![
                Just('A'), Just('z'), Just(' '), Just('\n'), Just('漢'), Just('字'),
                Just('\u{3000}'), Just('ア'), Just('1'),
            ],
            0..40,
        ),
        ci in 0usize..4,
    ) {
        let text: String = chars.into_iter().collect();
        let e = enc(Ccsid::ALL[ci], Records::Nl);
        let bytes = encode(e, &text);
        prop_assert_eq!(decode_str(e, &bytes), text);
    }
}
