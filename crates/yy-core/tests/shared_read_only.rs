use yy_core::{Document, OpenOptions};

#[test]
fn shared_file_is_read_only_and_keeps_its_opening_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shared.txt");
    std::fs::write(&path, b"hello\n").unwrap();
    let mut doc = Document::open_shared_read_only(&path, &OpenOptions::default()).unwrap();
    assert!(doc.is_read_only());
    assert_eq!(doc.snapshot().read(0..doc.snapshot().len()), b"hello\n");
    std::fs::write(&path, b"changed\n").unwrap();
    assert_eq!(doc.snapshot().read(0..doc.snapshot().len()), b"hello\n");
    assert!(!doc.insert_text("!", false));
    assert!(!doc.undo());
    assert!(doc.save().is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"changed\n");
}

/// 大きなファイルは先頭部分を表示しておき、コピー（と変換）をバックグラウンドで行う。
#[test]
fn large_shared_file_is_copied_in_the_background() {
    let dir = tempfile::tempdir().unwrap();
    let pool = yy_jobs::JobPool::new(2);
    let text = "行 0123456789 abcdefghij\n".repeat(100_000);
    for enc in [yy_core::Encoding::Utf8, yy_core::Encoding::Cp932] {
        let path = dir.path().join("big.txt");
        let bytes: Vec<u8> = if enc == yy_core::Encoding::Utf8 {
            b"\xEF\xBB\xBF"
                .iter()
                .chain(text.as_bytes())
                .copied()
                .collect()
        } else {
            // 「行」は Shift_JIS で 0x8D73
            b"\x8D\x73 0123456789 abcdefghij\n".repeat(100_000)
        };
        std::fs::write(&path, &bytes).unwrap();
        let mut doc = Document::open_shared_read_only(
            &path,
            &OpenOptions {
                encoding: Some(enc),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(doc.is_loading(), "{enc}");
        assert!(doc.snapshot().len() < text.len() as u64);
        assert_eq!(doc.file_len(), bytes.len() as u64);
        doc.start_indexing(&pool, std::sync::Arc::new(|| {}));
        doc.wait_loading();
        assert!(doc.load_error().is_none());
        assert!(doc.is_read_only());
        assert_eq!(doc.has_bom(), enc == yy_core::Encoding::Utf8);
        assert_eq!(
            doc.snapshot().read(0..doc.snapshot().len()),
            text.as_bytes()
        );
    }
}

#[cfg(windows)]
#[test]
fn opens_a_file_held_open_for_writing_by_another_application() {
    use std::os::windows::fs::OpenOptionsExt;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("in-use.txt");
    std::fs::write(&path, b"in use\n").unwrap();
    let writer = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(0x1 | 0x2 | 0x4)
        .open(&path)
        .unwrap();
    assert!(Document::open(&path).is_err());
    let doc = Document::open_shared_read_only(&path, &OpenOptions::default()).unwrap();
    assert!(doc.is_read_only());
    assert_eq!(doc.snapshot().read(0..doc.snapshot().len()), b"in use\n");
    drop(writer);
}
