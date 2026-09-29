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
