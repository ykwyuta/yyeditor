//! 区切り文字モード（04 章）。

use std::sync::Arc;

use yy_core::csv::{CsvView, RecordOp};
use yy_core::{Document, Selection, SelectionSet};
use yy_delimited::{Dialect, LineState, RecordIndex};
use yy_jobs::JobPool;

fn text(d: &Document) -> String {
    String::from_utf8(d.snapshot().read(0..d.snapshot().len())).unwrap()
}

#[test]
fn column_operations_are_one_undo_step() {
    let src = "a,b,c\n1,\"x\ny\",3\n";
    let mut d = Document::from_text(src);
    let pool = JobPool::new(1);
    for sync_limit in [u64::MAX, 0] {
        d.set_replace_sync_limit(sync_limit);
        let r = d
            .transform_records(
                Dialect::csv(),
                RecordOp::DeleteField(1),
                &pool,
                Arc::new(|| {}),
            )
            .unwrap();
        if r.is_none() {
            while d.is_busy() {
                d.poll_indexing();
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            assert_eq!(d.take_replace_result(), Some(Ok(2)));
        }
        assert_eq!(text(&d), "a,c\n1,3\n");
        assert!(d.undo());
        assert_eq!(text(&d), src);
    }
}

/// 編集のたびにインデックスを合わせると、最初から作り直したものと同じ結果になる。
#[test]
fn index_follows_edits() {
    let mut d = Document::from_text(&"x,\"a\nb\",y\n".repeat(200));
    let pool = JobPool::new(1);
    let mut view = CsvView::new(Dialect::csv());
    view.sync(d.snapshot(), &pool, Arc::new(|| {}));
    for (pos, t) in [(3, "\""), (500, "\"q"), (10, "z\nz"), (0, "\"")] {
        d.set_selections(SelectionSet::single(Selection::caret(pos)));
        d.insert_text(t, false);
        view.sync(d.snapshot(), &pool, Arc::new(|| {}));
        let snap = d.snapshot();
        let mut fresh = RecordIndex::new(Dialect::csv());
        fresh.extend(snap, u64::MAX);
        let idx = view.index();
        let idx = idx.lock().unwrap();
        let mut line = 0;
        while let Some(nl) = snap.find_next(line..snap.len(), b'\n') {
            line = nl + 1;
            let expect: LineState = fresh.line_state_at(snap, line);
            assert_eq!(
                idx.line_state_at(snap, line),
                expect,
                "after {t:?} at {line}"
            );
        }
    }
}
