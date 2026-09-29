//! バックグラウンドジョブ基盤。
//!
//! ファイルサイズに比例する処理（行数カウント、文字コード変換、検索、保存など）を
//! ワーカースレッドで実行し、進捗の取得とキャンセルを提供する。
//! 詳細は `docs/proposal/01-architecture.md` 4.2 を参照。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;

use crossbeam_channel::{Receiver, Sender, unbounded};

/// キャンセル要求を伝えるトークン。ジョブはチャンク処理ごとに確認する。
#[derive(Clone, Debug, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// ジョブの進捗（処理済み量 / 総量）。単位はジョブが決める（通常はバイト数）。
#[derive(Debug, Default)]
pub struct Progress {
    done: AtomicU64,
    total: AtomicU64,
}

impl Progress {
    pub fn set_total(&self, total: u64) {
        self.total.store(total, Ordering::Relaxed);
    }

    pub fn set_done(&self, done: u64) {
        self.done.store(done, Ordering::Relaxed);
    }

    pub fn add_done(&self, n: u64) {
        self.done.fetch_add(n, Ordering::Relaxed);
    }

    pub fn done(&self) -> u64 {
        self.done.load(Ordering::Relaxed)
    }

    pub fn total(&self) -> u64 {
        self.total.load(Ordering::Relaxed)
    }

    /// 0.0〜1.0 の進捗率。総量が 0 なら 0.0。
    pub fn fraction(&self) -> f64 {
        let t = self.total();
        if t == 0 {
            0.0
        } else {
            (self.done() as f64 / t as f64).min(1.0)
        }
    }
}

/// 実行中のジョブに渡される文脈。
pub struct JobContext {
    pub cancel: CancelToken,
    pub progress: Arc<Progress>,
}

/// UI スレッドを起こすための通知関数（Win32 では `PostMessageW` を呼ぶ）。
pub type Notifier = Arc<dyn Fn() + Send + Sync>;

type Task = Box<dyn FnOnce() + Send>;

/// 投入したジョブへの参照。ドロップしてもジョブは止まらない（止めるには [`JobHandle::cancel`]）。
#[derive(Clone)]
pub struct JobHandle {
    cancel: CancelToken,
    progress: Arc<Progress>,
    finished: Arc<AtomicBool>,
}

impl JobHandle {
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    pub fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }

    pub fn progress(&self) -> &Progress {
        &self.progress
    }
}

/// 固定数のワーカースレッドによるジョブプール。
pub struct JobPool {
    tx: Option<Sender<Task>>,
    workers: Vec<JoinHandle<()>>,
}

impl JobPool {
    /// `threads` 本のワーカーを起動する（0 なら CPU 数に合わせる）。
    pub fn new(threads: usize) -> JobPool {
        let threads = if threads == 0 {
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(2)
                .clamp(2, 8)
        } else {
            threads
        };
        let (tx, rx): (Sender<Task>, Receiver<Task>) = unbounded();
        let workers = (0..threads)
            .map(|i| {
                let rx = rx.clone();
                std::thread::Builder::new()
                    .name(format!("yy-job-{i}"))
                    .spawn(move || {
                        while let Ok(task) = rx.recv() {
                            task();
                        }
                    })
                    .expect("failed to spawn worker thread")
            })
            .collect();
        JobPool {
            tx: Some(tx),
            workers,
        }
    }

    pub fn threads(&self) -> usize {
        self.workers.len()
    }

    /// ジョブを投入する。ジョブ内でパニックしてもワーカーは停止しない。
    pub fn spawn<F>(&self, f: F) -> JobHandle
    where
        F: FnOnce(&JobContext) + Send + 'static,
    {
        let handle = JobHandle {
            cancel: CancelToken::new(),
            progress: Arc::new(Progress::default()),
            finished: Arc::new(AtomicBool::new(false)),
        };
        let ctx = JobContext {
            cancel: handle.cancel.clone(),
            progress: handle.progress.clone(),
        };
        let finished = handle.finished.clone();
        let task: Task = Box::new(move || {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&ctx)));
            finished.store(true, Ordering::Release);
        });
        self.tx
            .as_ref()
            .expect("pool is shut down")
            .send(task)
            .expect("worker threads are gone");
        handle
    }
}

impl Default for JobPool {
    fn default() -> Self {
        JobPool::new(0)
    }
}

impl Drop for JobPool {
    fn drop(&mut self) {
        self.tx.take();
        for w in self.workers.drain(..) {
            let _ = w.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn wait(h: &JobHandle) {
        let t = Instant::now();
        while !h.is_finished() {
            assert!(t.elapsed() < Duration::from_secs(10), "job did not finish");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn runs_jobs_and_reports_progress() {
        let pool = JobPool::new(2);
        let (tx, rx) = crossbeam_channel::unbounded();
        let h = pool.spawn(move |ctx| {
            ctx.progress.set_total(10);
            for i in 0..10u64 {
                ctx.progress.add_done(1);
                tx.send(i).unwrap();
            }
        });
        wait(&h);
        assert_eq!(rx.try_iter().count(), 10);
        assert_eq!(h.progress().fraction(), 1.0);
    }

    #[test]
    fn cancellation_is_observed() {
        let pool = JobPool::new(1);
        let (started_tx, started_rx) = crossbeam_channel::bounded(1);
        let h = pool.spawn(move |ctx| {
            started_tx.send(()).unwrap();
            while !ctx.cancel.is_cancelled() {
                std::thread::yield_now();
            }
        });
        started_rx.recv().unwrap();
        h.cancel();
        wait(&h);
        assert!(h.is_cancelled());
    }

    #[test]
    fn panicking_job_does_not_kill_worker() {
        let pool = JobPool::new(1);
        let h1 = pool.spawn(|_| panic!("boom"));
        wait(&h1);
        let h2 = pool.spawn(|ctx| ctx.progress.set_done(1));
        wait(&h2);
        assert_eq!(h2.progress().done(), 1);
    }
}
