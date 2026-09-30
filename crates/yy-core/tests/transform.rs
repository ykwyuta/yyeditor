//! 選択範囲の変換と行の重複の除去（文書の操作として）。

use std::sync::Arc;

use yy_core::transform::Transform;
use yy_core::{Document, Selection, SelectionSet};
use yy_jobs::JobPool;

fn text(d: &Document) -> String {
    String::from_utf8(d.snapshot().read(0..d.snapshot().len())).unwrap()
}

#[test]
fn transforms_every_selection_and_keeps_them_selected() {
    let mut d = Document::from_text("fooBar and bazQux");
    d.set_selections(SelectionSet::from_vec(
        vec![Selection::new(0, 6), Selection::new(11, 17)],
        1,
    ));
    assert!(d.transform_selections(Transform::Snake));
    assert_eq!(text(&d), "foo_bar and baz_qux");
    let sels: Vec<_> = d.selections().iter().map(|s| s.range()).collect();
    assert_eq!(sels, vec![0..7, 12..19]);
    // 続けて別の変換
    assert!(d.transform_selections(Transform::Upper));
    assert_eq!(text(&d), "FOO_BAR and BAZ_QUX");
    assert!(d.undo());
    assert_eq!(text(&d), "foo_bar and baz_qux");
}

#[test]
fn empty_selection_transforms_the_word_at_the_caret() {
    let mut d = Document::from_text("x ｶﾀｶﾅ y");
    // 「ﾀ」の前（バイト位置 5）
    d.set_selections(SelectionSet::single(Selection::caret(5)));
    assert!(d.transform_selections(Transform::FullKatakana));
    assert_eq!(text(&d), "x カタカナ y");
    // 変わらなければ何もしない
    assert!(!d.transform_selections(Transform::FullKatakana));
}

#[test]
fn dedups_selected_lines_or_whole_document() {
    let pool = JobPool::new(2);
    let mut d = Document::from_text("a\nb\na\nc\nb\n");
    assert_eq!(d.dedup_lines(&pool, Arc::new(|| {})).unwrap(), Some(2));
    assert_eq!(text(&d), "a\nb\nc\n");
    assert!(d.undo());
    // 選択した行（3〜5 行目）の中だけ
    d.set_selections(SelectionSet::single(Selection::new(4, 10)));
    assert_eq!(d.dedup_lines(&pool, Arc::new(|| {})).unwrap(), Some(0));
    let mut d = Document::from_text("a\nb\nx\nx\nb\n");
    d.set_selections(SelectionSet::single(Selection::new(4, 9)));
    assert_eq!(d.dedup_lines(&pool, Arc::new(|| {})).unwrap(), Some(1));
    assert_eq!(text(&d), "a\nb\nx\nb\n");
}

#[test]
fn dedups_large_documents_in_the_background() {
    let pool = JobPool::new(2);
    let mut d = Document::from_text(&"same\nother\n".repeat(1000));
    d.set_replace_sync_limit(0);
    assert_eq!(d.dedup_lines(&pool, Arc::new(|| {})).unwrap(), None);
    while d.replace_progress().is_some() {
        if !d.poll_indexing() {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
    assert!(matches!(d.take_replace_result(), Some(Ok(1998))));
    assert_eq!(text(&d), "same\nother\n");
}
