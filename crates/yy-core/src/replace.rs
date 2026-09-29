//! すべて置換（05 章 4.2）。
//!
//! 置換する箇所が少なければ通常の一括編集として、多ければ文書全体を置換しながら新しい
//! 一時ファイル（大きな文書）かメモリ（小さな文書）に書き出して差し替える。
//! どちらも 1 回の Undo で元に戻せる。大きな文書ではバックグラウンドで実行する。

use std::io::{BufWriter, Write};
use std::ops::Range;
use std::sync::Arc;

use crossbeam_channel::{Receiver, bounded};
use yy_buffer::{Snapshot, SourceRef};
use yy_jobs::{JobHandle, JobPool, Notifier};
use yy_search::{ReplaceError, Replacement, Searcher, collect_edits, rewrite};

/// 一括編集にする置換の数の上限（これより多ければ書き直す）
pub(crate) const EDIT_LIMIT: usize = 10_000;
/// 書き直しをメモリ上で行う文書の大きさの上限
pub(crate) const IN_MEMORY: u64 = 64 << 20;

/// すべて置換の結果。
pub(crate) enum Outcome {
    /// 個々の置換（開始位置の昇順）
    Edits(Vec<(Range<u64>, Vec<u8>)>),
    /// 置き換えた数と、置換後の文書全体
    Rewritten(u64, Snapshot),
}

/// すべて置換を実行する（その場で）。
pub(crate) fn run(
    searcher: &Searcher,
    snap: &Snapshot,
    range: Range<u64>,
    repl: &Replacement,
    in_memory: u64,
    step: &mut dyn FnMut(u64) -> bool,
) -> Result<Outcome, ReplaceError> {
    if !step(range.start) {
        return Err(ReplaceError::Cancelled);
    }
    if let Some(edits) = collect_edits(searcher, snap, range.clone(), repl, EDIT_LIMIT, step)? {
        return Ok(Outcome::Edits(edits));
    }
    produce(snap.len(), in_memory, &mut |w| {
        rewrite(searcher, snap, range.clone(), repl, w, step)
    })
}

/// `write` が書き出した内容を新しい文書にする。`size_hint` が `in_memory` 以下ならメモリ上に、
/// それより大きければ一時ファイルに書き出してマップする。`write` は処理した数を返す。
pub(crate) fn produce(
    size_hint: u64,
    in_memory: u64,
    write: &mut dyn FnMut(&mut dyn Write) -> Result<u64, ReplaceError>,
) -> Result<Outcome, ReplaceError> {
    if size_hint <= in_memory {
        let mut out = Vec::with_capacity(size_hint as usize);
        let n = write(&mut out)?;
        return Ok(Outcome::Rewritten(n, Snapshot::from_bytes(out)));
    }
    let path = yy_io::temp_path("rewrite");
    let result = (|| -> Result<Outcome, ReplaceError> {
        let file = std::fs::File::create(&path)?;
        let mut w = BufWriter::with_capacity(1 << 20, file);
        let n = write(&mut w)?;
        w.flush()?;
        drop(w);
        let len = std::fs::metadata(&path)?.len();
        let snapshot = if len == 0 {
            let _ = std::fs::remove_file(&path);
            Snapshot::empty()
        } else {
            let map: SourceRef = yy_io::map_temp(&path)?;
            Snapshot::from_source(map, 0..len, false)
        };
        Ok(Outcome::Rewritten(n, snapshot))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&path);
    }
    result
}

/// バックグラウンドで実行する書き直し（`step(進んだ位置)` が `false` なら中止）。
pub(crate) type Task =
    Box<dyn FnOnce(&mut dyn FnMut(u64) -> bool) -> Result<Outcome, ReplaceError> + Send>;

/// バックグラウンドのすべて置換。ドロップすると中止する。
pub(crate) struct ReplaceJob {
    job: JobHandle,
    rx: Receiver<Result<Outcome, ReplaceError>>,
}

impl ReplaceJob {
    pub fn start(
        pool: &JobPool,
        notify: Notifier,
        searcher: Arc<Searcher>,
        repl: Replacement,
        snap: Snapshot,
        range: Range<u64>,
        in_memory: u64,
    ) -> ReplaceJob {
        let total = range.clone();
        ReplaceJob::start_task(
            pool,
            notify,
            total,
            Box::new(move |step| run(&searcher, &snap, range, &repl, in_memory, step)),
        )
    }

    /// 任意の書き直しをバックグラウンドで実行する。`range` は進捗の範囲。
    pub fn start_task(
        pool: &JobPool,
        notify: Notifier,
        range: Range<u64>,
        task: Task,
    ) -> ReplaceJob {
        let (tx, rx) = bounded(1);
        let job = pool.spawn(move |ctx| {
            ctx.progress.set_total(range.end - range.start);
            let start = range.start;
            let mut step = |pos: u64| {
                ctx.progress.set_done(pos.saturating_sub(start));
                !ctx.cancel.is_cancelled()
            };
            let r = task(&mut step);
            let _ = tx.send(r);
            notify();
        });
        ReplaceJob { job, rx }
    }

    pub fn poll(&self) -> Option<Result<Outcome, ReplaceError>> {
        self.rx.try_recv().ok()
    }

    pub fn progress(&self) -> f64 {
        self.job.progress().fraction()
    }

    pub fn cancel(&self) {
        self.job.cancel();
    }
}

impl Drop for ReplaceJob {
    fn drop(&mut self) {
        self.job.cancel();
    }
}
