//! 改行数のバックグラウンドカウント（Indexer ジョブ）。02 章 3.4 参照。

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, unbounded};
use yy_buffer::{Piece, PieceKey, Snapshot, count_lf};
use yy_jobs::{JobHandle, JobPool, Notifier};

/// 数え終わったピースの改行数。
#[derive(Debug, Default)]
pub struct IndexBatch {
    pub counts: Vec<(PieceKey, u32)>,
}

/// 実行中の改行カウント。ドロップするとキャンセルされる。
pub struct Indexer {
    jobs: Vec<JobHandle>,
    rx: Receiver<IndexBatch>,
    done_bytes: Arc<AtomicU64>,
    total_bytes: u64,
}

/// 1 回の通知で送るバイト数・時間の目安。
const BATCH_BYTES: u64 = 64 << 20;
const BATCH_INTERVAL: Duration = Duration::from_millis(50);

impl Indexer {
    /// `snapshot` の未確定ピースを数えるジョブを起動する。
    ///
    /// ピース列を連続した区間に分けて並列に数える（ページフォルトの待ちを重ねるため）。
    /// 結果が溜まるたびに `notify` を呼ぶ。
    pub fn start(pool: &JobPool, snapshot: &Snapshot, notify: Notifier) -> Indexer {
        let pieces = snapshot.unindexed_pieces(usize::MAX);
        let total_bytes: u64 = pieces.iter().map(|p| p.len() as u64).sum();
        let (tx, rx) = unbounded();
        let done_bytes = Arc::new(AtomicU64::new(0));
        let parts = pool.threads().clamp(1, 4).min(pieces.len().max(1));
        let per = pieces.len().div_ceil(parts).max(1);
        let mut jobs = Vec::with_capacity(parts);
        for group in pieces.chunks(per) {
            let group: Vec<Piece> = group.to_vec();
            let tx = tx.clone();
            let notify = notify.clone();
            let done_bytes = done_bytes.clone();
            jobs.push(pool.spawn(move |ctx| {
                let group_total: u64 = group.iter().map(|p| p.len() as u64).sum();
                ctx.progress.set_total(group_total);
                let mut batch = IndexBatch::default();
                let mut batch_bytes = 0u64;
                let mut last_sent = Instant::now();
                for p in &group {
                    if ctx.cancel.is_cancelled() {
                        return;
                    }
                    batch.counts.push((p.key(), count_lf(p.bytes())));
                    let n = p.len() as u64;
                    batch_bytes += n;
                    ctx.progress.add_done(n);
                    done_bytes.fetch_add(n, Ordering::Relaxed);
                    if batch_bytes >= BATCH_BYTES || last_sent.elapsed() >= BATCH_INTERVAL {
                        if tx.send(std::mem::take(&mut batch)).is_err() {
                            return;
                        }
                        notify();
                        batch_bytes = 0;
                        last_sent = Instant::now();
                    }
                }
                if !batch.counts.is_empty() && tx.send(batch).is_ok() {
                    notify();
                }
            }));
        }
        if jobs.is_empty() {
            notify();
        }
        Indexer {
            jobs,
            rx,
            done_bytes,
            total_bytes,
        }
    }

    /// 届いている結果をすべて取り出す。
    pub fn drain(&self) -> Vec<IndexBatch> {
        self.rx.try_iter().collect()
    }

    pub fn is_finished(&self) -> bool {
        self.jobs.iter().all(|j| j.is_finished()) && self.rx.is_empty()
    }

    pub fn cancel(&self) {
        for j in &self.jobs {
            j.cancel();
        }
    }

    /// 0.0〜1.0 の進捗率。
    pub fn progress(&self) -> f64 {
        if self.total_bytes == 0 {
            1.0
        } else {
            self.done_bytes.load(Ordering::Relaxed) as f64 / self.total_bytes as f64
        }
    }
}

impl Drop for Indexer {
    fn drop(&mut self) {
        self.cancel();
    }
}
