//! 置換（05 章 4）。

use std::sync::Arc;

use yy_core::{Document, Query, Replacement, Searcher, Selection, SelectionSet};
use yy_jobs::JobPool;

fn text(d: &Document) -> String {
    String::from_utf8(d.snapshot().read(0..d.snapshot().len())).unwrap()
}

fn searcher(p: &str, regex: bool) -> Arc<Searcher> {
    Arc::new(
        Searcher::new(&Query {
            pattern: p.into(),
            regex,
            case_sensitive: true,
            whole_word: false,
        })
        .unwrap(),
    )
}

fn replace_all(d: &mut Document, s: &Arc<Searcher>, r: Replacement) -> u64 {
    let pool = JobPool::new(2);
    let len = d.snapshot().len();
    match d
        .replace_all(s.clone(), r, 0..len, &pool, Arc::new(|| {}))
        .unwrap()
    {
        Some(n) => n,
        None => {
            assert!(d.is_busy());
            assert!(!d.insert_text("x", false));
            while d.is_busy() {
                d.poll_indexing();
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            d.take_replace_result().unwrap().unwrap()
        }
    }
}

#[test]
fn replace_all_few_matches_is_one_undo_step() {
    let mut d = Document::from_text("foo bar foo\nbaz foo");
    let s = searcher("foo", false);
    assert_eq!(replace_all(&mut d, &s, Replacement::literal("qux")), 3);
    assert_eq!(text(&d), "qux bar qux\nbaz qux");
    assert!(d.undo());
    assert_eq!(text(&d), "foo bar foo\nbaz foo");
}

#[test]
fn replace_all_with_captures() {
    let mut d = Document::from_text("2026-09-29\n1999-01-02\n");
    let s = searcher(r"(\d+)-(\d+)-(\d+)", true);
    let r = Replacement::parse("$3/$2/$1", &s).unwrap();
    assert_eq!(replace_all(&mut d, &s, r), 2);
    assert_eq!(text(&d), "29/09/2026\n02/01/1999\n");
}

#[test]
fn many_matches_are_rewritten() {
    let line = "key=value; key=other\n";
    let src = line.repeat(20_000);
    for sync_limit in [u64::MAX, 0] {
        let mut d = Document::from_text(&src);
        d.set_replace_sync_limit(sync_limit);
        let s = searcher("key=", false);
        assert_eq!(replace_all(&mut d, &s, Replacement::literal("k:")), 40_000);
        assert_eq!(text(&d), src.replace("key=", "k:"));
        assert!(d.undo());
        assert_eq!(text(&d), src);
        assert!(d.redo());
        assert_eq!(text(&d), src.replace("key=", "k:"));
    }
}

#[test]
fn background_replace_can_be_cancelled() {
    let src = "abc\n".repeat(200_000);
    let mut d = Document::from_text(&src);
    d.set_replace_sync_limit(0);
    let pool = JobPool::new(1);
    // ジョブを始める前にプールを埋めておき、確実に中止されるようにする
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    pool.spawn(move |_| {
        let _ = rx.recv();
    });
    let s = searcher("b", false);
    let len = d.snapshot().len();
    let r = d
        .replace_all(s, Replacement::literal("x"), 0..len, &pool, Arc::new(|| {}))
        .unwrap();
    assert!(r.is_none());
    d.cancel_replace();
    tx.send(()).unwrap();
    while d.is_busy() {
        d.poll_indexing();
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(d.take_replace_result().unwrap().is_err());
    assert_eq!(text(&d), src);
    assert!(!d.can_undo());
}

#[test]
fn replace_selection_only_when_it_matches() {
    let mut d = Document::from_text("one two one");
    let s = searcher("one", false);
    let r = Replacement::literal("1");
    d.set_selections(SelectionSet::single(Selection::new(0, 2)));
    assert!(!d.replace_selection(&s, &r));
    d.set_selections(SelectionSet::single(Selection::new(8, 11)));
    assert!(d.replace_selection(&s, &r));
    assert_eq!(text(&d), "one two 1");
    assert_eq!(d.selections().primary().head, 9);
}

#[test]
fn replace_within_range() {
    let mut d = Document::from_text("aaa|aaa|aaa");
    let s = searcher("a", false);
    let pool = JobPool::new(1);
    let n = d
        .replace_all(s, Replacement::literal("b"), 4..7, &pool, Arc::new(|| {}))
        .unwrap();
    assert_eq!(n, Some(3));
    assert_eq!(text(&d), "aaa|bbb|aaa");
}
