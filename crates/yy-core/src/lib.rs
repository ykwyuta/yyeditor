//! 文書モデル。
//!
//! M1 では「開く・行数をバックグラウンドで数える・表示用に内容を提供する」までを扱う。
//! 編集・選択・Undo は M2 で追加する（01 章 4.3）。

mod indexer;

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

pub use indexer::{IndexBatch, Indexer};
use yy_buffer::Snapshot;
use yy_io::{Bom, FileGuard};
use yy_jobs::{JobPool, Notifier};

pub struct Document {
    path: Option<PathBuf>,
    file_len: u64,
    bom: Bom,
    snapshot: Snapshot,
    /// 内容が変わるたびに増える（M2 以降）
    generation: u64,
    /// 行数の情報が増えるたびに増える（表示の行番号の更新判定用）
    index_version: u64,
    indexer: Option<Indexer>,
    _guard: Option<FileGuard>,
}

impl Default for Document {
    fn default() -> Self {
        Document::new_empty()
    }
}

impl Document {
    pub fn new_empty() -> Document {
        Document {
            path: None,
            file_len: 0,
            bom: Bom::None,
            snapshot: Snapshot::empty(),
            generation: 0,
            index_version: 0,
            indexer: None,
            _guard: None,
        }
    }

    /// ファイルを開く。行数のカウントは [`Document::start_indexing`] で別途開始する。
    pub fn open(path: &Path) -> io::Result<Document> {
        let o = yy_io::open_file(path)?;
        // 先頭のピース（最初の画面で必ず読む範囲）だけはその場で数えておく。
        // 未確定範囲の行数を推定するときの改行密度として使う。
        let first: Vec<_> = o.snapshot.unindexed_pieces(1);
        let first_key = first
            .first()
            .map(|p| (p.key(), yy_buffer::count_lf(p.bytes())));
        let snapshot = match first_key {
            Some((key, n)) => o
                .snapshot
                .fill_line_counts(&|p| (p.key() == key).then_some(n)),
            None => o.snapshot,
        };
        Ok(Document {
            path: Some(o.path),
            file_len: o.file_len,
            bom: o.bom,
            snapshot,
            generation: 0,
            index_version: 0,
            indexer: None,
            _guard: Some(o.guard),
        })
    }

    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// タイトルバー等に表示する名前。
    pub fn display_name(&self) -> String {
        self.path
            .as_deref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "無題".to_owned())
    }

    pub fn file_len(&self) -> u64 {
        self.file_len
    }

    pub fn bom(&self) -> Bom {
        self.bom
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn index_version(&self) -> u64 {
        self.index_version
    }

    /// 改行数のバックグラウンドカウントを開始する。
    pub fn start_indexing(&mut self, pool: &JobPool, notify: Notifier) {
        if self.snapshot.is_fully_indexed() {
            return;
        }
        self.indexer = Some(Indexer::start(pool, &self.snapshot, notify));
    }

    /// 届いたカウント結果を文書に反映する。反映したら `true`。
    pub fn poll_indexing(&mut self) -> bool {
        let Some(indexer) = &self.indexer else {
            return false;
        };
        let batches = indexer.drain();
        let finished = indexer.is_finished();
        if finished {
            self.indexer = None;
        }
        if batches.is_empty() {
            return finished;
        }
        let map: HashMap<_, _> = batches.into_iter().flat_map(|b| b.counts).collect();
        self.snapshot = self
            .snapshot
            .fill_line_counts(&|p| map.get(&p.key()).copied());
        self.index_version += 1;
        true
    }

    /// 行数カウントの進捗率。カウント中でなければ `None`。
    pub fn indexing_progress(&self) -> Option<f64> {
        self.indexer.as_ref().map(|i| i.progress())
    }

    /// 行数を数え終わるまで待つ（テスト・ベンチマーク用）。
    pub fn wait_indexing(&mut self) {
        while self.indexer.is_some() {
            if !self.poll_indexing() {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn indexes_file_in_background() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        let mut data = Vec::new();
        let mut i = 0u64;
        while data.len() < 6 << 20 {
            writeln!(data, "line {i} あいうえお").unwrap();
            i += 1;
        }
        f.write_all(&data).unwrap();

        let mut doc = Document::open(f.path()).unwrap();
        assert_eq!(doc.snapshot().line_count(), None);
        let pool = JobPool::new(3);
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        doc.start_indexing(
            &pool,
            Arc::new(move || {
                c.fetch_add(1, Ordering::Relaxed);
            }),
        );
        assert!(doc.indexing_progress().is_some());
        doc.wait_indexing();
        assert!(calls.load(Ordering::Relaxed) > 0);
        assert_eq!(doc.snapshot().line_count(), Some(i + 1));
        assert!(doc.index_version() > 0);
        assert!(doc.indexing_progress().is_none());
        doc.snapshot().check_invariants();
    }

    #[test]
    fn empty_document() {
        let doc = Document::new_empty();
        assert_eq!(doc.display_name(), "無題");
        assert_eq!(doc.snapshot().line_count(), Some(1));
    }
}
