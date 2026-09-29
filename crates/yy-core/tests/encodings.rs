//! 文字コードを指定した読み込みと保存（M3 の完了条件: 「開く→保存」でバイト一致）。

use std::path::Path;
use std::sync::Arc;

use yy_core::{Document, Encoding, EscapeMode, OpenOptions, SaveError};
use yy_jobs::JobPool;

const SAMPLE: &str =
    "日本語のテキスト、ひらがな・カタカナ・漢字。\r\nABC ｱｲｳ ①Ⅱ㈱ 〜～−\r\n最後の行";

fn encodings() -> Vec<Encoding> {
    let mut v = vec![
        Encoding::Utf8,
        Encoding::Utf16Le,
        Encoding::Utf16Be,
        Encoding::Utf32Le,
        Encoding::Utf32Be,
        Encoding::ShiftJis,
        Encoding::Cp932,
        Encoding::ShiftJis2004,
        Encoding::EucJp,
        Encoding::EucJis2004,
        Encoding::Iso2022Jp,
    ];
    v.push(Encoding::from_name("windows-1252").unwrap());
    v
}

/// `enc` で表せる範囲のサンプルを `enc` でエンコードし、不正なバイトを混ぜる。
fn sample_bytes(enc: Encoding) -> Vec<u8> {
    let text: String = SAMPLE
        .chars()
        .filter(|c| {
            yy_encoding::encode_all(enc, c.to_string().as_bytes(), EscapeMode::Reject).is_ok()
        })
        .collect();
    let mut bytes = yy_encoding::encode_all(enc, text.as_bytes(), EscapeMode::Reject).unwrap();
    if enc != Encoding::Iso2022Jp && enc != Encoding::Utf8 {
        // その文字コードとして不正なバイト（または UTF-16/32 の不対サロゲート）
        let junk: &[u8] = match enc {
            Encoding::Utf16Le => b"\x00\xD8",
            Encoding::Utf16Be => b"\xDC\x00",
            Encoding::Utf32Le | Encoding::Utf32Be => b"\xFF\xFF\xFF\xFF",
            _ => b"\xFF\x80\xA0",
        };
        bytes.extend_from_slice(junk);
    }
    if enc == Encoding::Utf8 {
        bytes.extend_from_slice(b"\xFF\xC0");
    }
    bytes
}

fn open(path: &Path, enc: Option<Encoding>, sync_limit: u64) -> Document {
    let pool = JobPool::new(2);
    let mut d = Document::open_with(
        path,
        &OpenOptions {
            encoding: enc,
            sync_limit,
        },
    )
    .unwrap();
    d.start_indexing(&pool, Arc::new(|| {}));
    d.wait_loading();
    assert!(d.load_error().is_none());
    d
}

#[test]
fn every_encoding_roundtrips_byte_for_byte() {
    let dir = tempfile::tempdir().unwrap();
    for enc in encodings() {
        for (bom, sync_limit) in [(false, u64::MAX), (false, 0), (true, u64::MAX)] {
            if bom && !enc.supports_bom() {
                continue;
            }
            let path = dir.path().join("file.txt");
            let mut bytes = if bom { enc.bom().to_vec() } else { Vec::new() };
            bytes.extend(sample_bytes(enc));
            std::fs::write(&path, &bytes).unwrap();
            let mut d = open(&path, Some(enc), sync_limit);
            assert_eq!(d.encoding(), enc);
            assert_eq!(d.has_bom(), bom, "{enc}");
            d.save().unwrap_or_else(|e| panic!("{enc}: {e}"));
            assert_eq!(
                std::fs::read(&path).unwrap(),
                bytes,
                "{enc} bom={bom} limit={sync_limit}"
            );
            // 保存したファイルを開き直しても同じ内容
            let text = d.snapshot().read(0..d.snapshot().len());
            drop(d);
            let d2 = open(&path, Some(enc), sync_limit);
            assert_eq!(d2.snapshot().read(0..d2.snapshot().len()), text, "{enc}");
        }
    }
}

#[test]
fn detects_encoding_on_open() {
    let dir = tempfile::tempdir().unwrap();
    for enc in [
        Encoding::Utf8,
        Encoding::Cp932,
        Encoding::EucJp,
        Encoding::Iso2022Jp,
        Encoding::Utf16Le,
    ] {
        let path = dir.path().join("auto.txt");
        let mut bytes = if enc == Encoding::Utf16Le {
            enc.bom().to_vec()
        } else {
            Vec::new()
        };
        // 〜 − は CP932 などで全角チルダ・全角マイナスに変わるので除く
        let text = SAMPLE.replace(['〜', '−'], "").repeat(3);
        bytes.extend(yy_encoding::encode_all(enc, text.as_bytes(), EscapeMode::Literal).unwrap());
        std::fs::write(&path, &bytes).unwrap();
        let d = open(&path, None, u64::MAX);
        assert_eq!(d.encoding(), enc);
        if enc != Encoding::Iso2022Jp {
            // ISO-2022-JP は半角カナを全角にする
            assert_eq!(
                d.snapshot().read(0..d.snapshot().len()),
                text.as_bytes(),
                "{enc}"
            );
        }
    }
}

#[test]
fn converts_encoding_on_save_as() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("sjis.txt");
    std::fs::write(&src, b"\x93\xFA\x96\x7B\x8C\xEA\r\n").unwrap();
    let mut d = open(&src, None, u64::MAX);
    assert_eq!(d.encoding(), Encoding::Cp932);
    let dst = dir.path().join("utf8.txt");
    d.save_as_with(&dst, Encoding::Utf8, true).unwrap();
    assert_eq!(
        std::fs::read(&dst).unwrap(),
        "\u{FEFF}日本語\r\n".as_bytes()
    );
    assert_eq!(d.encoding(), Encoding::Utf8);
    assert!(d.has_bom());
    // UTF-16BE にも
    let dst16 = dir.path().join("utf16.txt");
    d.save_as_with(&dst16, Encoding::Utf16Be, false).unwrap();
    assert_eq!(
        std::fs::read(&dst16).unwrap(),
        b"\x65\xE5\x67\x2C\x8A\x9E\x00\r\x00\n"
    );
}

#[test]
fn unmappable_characters_can_be_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.txt");
    // 不正なバイト 0xFF を含む CP932
    std::fs::write(&path, b"A\xFFB\x82\xA0").unwrap();
    let mut d = open(&path, Some(Encoding::Cp932), u64::MAX);
    d.set_selections(yy_core::SelectionSet::single(yy_core::Selection::caret(
        d.snapshot().len(),
    )));
    d.insert_text("😀", false);
    let out = dir.path().join("b.txt");
    let err = d.save_as_with(&out, Encoding::EucJp, false).unwrap_err();
    let SaveError::Unmappable { ranges, total, .. } = err else {
        panic!("{err}");
    };
    // 不正なバイト（別の文字コードでは元に戻せない）と絵文字
    assert_eq!(total, 2);
    assert_eq!(ranges, vec![1..5, 9..13]);
    assert!(!out.exists());
    // 数値文字参照に置き換えれば保存できる（1 回の Undo で戻せる）
    assert!(d.replace_ranges(&ranges, |b| {
        match std::str::from_utf8(b).ok().and_then(|s| s.chars().next()) {
            Some(c) if yy_encoding::unescape_char(c).is_none() => {
                format!("&#x{:X};", c as u32).into_bytes()
            }
            _ => b"?".to_vec(),
        }
    }));
    d.save_as_with(&out, Encoding::EucJp, false).unwrap();
    assert_eq!(std::fs::read(&out).unwrap(), b"A?B\xA4\xA2&#x1F600;");
    // 同じ文字コードで上書き保存するならエスケープ文字は元のバイトに戻る
    assert!(d.undo());
    d.save_as_with(&path, Encoding::Cp932, false).unwrap_err();
}

#[test]
fn same_encoding_restores_invalid_bytes_after_edit() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.txt");
    std::fs::write(&path, b"\x82\xA0\xFF\x87\x40").unwrap();
    let mut d = open(&path, Some(Encoding::ShiftJis), u64::MAX);
    assert_eq!(d.decode_stats().invalid, 3);
    d.set_selections(yy_core::SelectionSet::single(yy_core::Selection::caret(0)));
    d.insert_text("い", false);
    d.save().unwrap();
    assert_eq!(
        std::fs::read(&path).unwrap(),
        b"\x82\xA2\x82\xA0\xFF\x87\x40"
    );
}

#[test]
fn background_loading_is_read_only_until_done() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("big.txt");
    let line = yy_encoding::encode_all(
        Encoding::Cp932,
        "0123456789 日本語の行です。\r\n".as_bytes(),
        EscapeMode::Reject,
    )
    .unwrap();
    let bytes = line.repeat(200_000);
    std::fs::write(&path, &bytes).unwrap();
    let mut d = Document::open_with(
        &path,
        &OpenOptions {
            encoding: None,
            sync_limit: 1 << 20,
        },
    )
    .unwrap();
    assert!(d.is_loading());
    assert!(d.snapshot().len() <= 2 << 20);
    assert!(!d.insert_text("x", false));
    assert!(d.save().is_err());
    let pool = JobPool::new(2);
    d.start_indexing(&pool, Arc::new(|| {}));
    d.wait_loading();
    assert!(!d.is_loading());
    assert!(!d.is_modified());
    d.start_indexing(&pool, Arc::new(|| {}));
    d.wait_indexing();
    assert_eq!(d.snapshot().line_count(), Some(200_001));
    assert!(d.insert_text("x", false));
    d.save().unwrap();
    let mut expect = b"x".to_vec();
    expect.extend_from_slice(&bytes);
    assert_eq!(std::fs::read(&path).unwrap(), expect);
}

#[test]
fn literal_escape_characters_disable_escaping() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.txt");
    // UTF-16LE で U+10FE41（エスケープ文字と同じ文字）を含む
    let text = format!("a{}", yy_encoding::escape_char(0x41));
    let bytes =
        yy_encoding::encode_all(Encoding::Utf16Le, text.as_bytes(), EscapeMode::Literal).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    let mut d = open(&path, Some(Encoding::Utf16Le), u64::MAX);
    assert_eq!(d.decode_stats().literal_escapes, 1);
    d.save().unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

#[test]
fn converts_line_endings_as_one_undo_step() {
    let mut d = Document::from_text("a\r\nb\nc\rd\r\n");
    d.set_selections(yy_core::SelectionSet::single(yy_core::Selection::caret(5)));
    assert!(d.convert_eol(yy_core::Eol::Lf).unwrap());
    assert_eq!(d.snapshot().read(0..d.snapshot().len()), b"a\nb\nc\rd\n");
    assert_eq!(d.selections().primary().head, 4);
    assert!(d.convert_eol(yy_core::Eol::CrLf).unwrap());
    assert_eq!(
        d.snapshot().read(0..d.snapshot().len()),
        b"a\r\nb\r\nc\rd\r\n"
    );
    assert!(!d.convert_eol(yy_core::Eol::CrLf).unwrap());
    assert!(d.undo());
    assert_eq!(d.snapshot().read(0..d.snapshot().len()), b"a\nb\nc\rd\n");
}
