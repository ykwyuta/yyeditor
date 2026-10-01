//! UTF-8 以外のファイルの読み込み（03 章、02 章 4）。
//!
//! 文書は UTF-8 で持つため、UTF-8 以外のファイルは開くときにデコードする。
//! 小さいファイルはその場でメモリに、大きいファイルはバックグラウンドで UTF-8 の一時ファイルに
//! 変換してメモリマップする（変換中は先頭部分を読み取り専用で表示する）。
//!
//! 他のアプリケーションが書き込み中のファイル（読み取り専用で開く）は、作業用ファイルへの
//! コピーもバックグラウンドで行う。

use std::fs::OpenOptions;
use std::io::{self, BufWriter, Write};
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;

use crossbeam_channel::{Receiver, bounded};
use yy_buffer::{ByteSource, Snapshot, SourceRef};
use yy_encoding::{DecodeStats, Encoding, EscapeMode};
use yy_io::MmapSource;
use yy_jobs::{JobHandle, JobPool, Notifier};

/// デコードの結果。
pub(crate) struct Decoded {
    pub snapshot: Snapshot,
    pub stats: DecodeStats,
    /// 保存時のエスケープ文字の扱い（元からエスケープ文字と同じ文字を含む場合は
    /// エスケープを使わずにデコードし直しているので `Literal`）
    pub escapes: EscapeMode,
}

/// メモリ上でデコードする。
pub(crate) fn decode_in_memory(enc: Encoding, bytes: &[u8]) -> Decoded {
    let (mut text, mut stats) = yy_encoding::decode_all(enc, bytes, true);
    let mut escapes = EscapeMode::Restore;
    if stats.literal_escapes > 0 {
        (text, stats) = yy_encoding::decode_all(enc, bytes, false);
        escapes = EscapeMode::Literal;
    }
    Decoded {
        snapshot: Snapshot::from_bytes(text),
        stats,
        escapes,
    }
}

/// 変換中に表示する先頭部分。
pub(crate) fn preview(enc: Encoding, bytes: &[u8], max: usize) -> Snapshot {
    let mut d = enc.new_decoder(true);
    let mut out = Vec::new();
    d.decode(&bytes[..bytes.len().min(max)], &mut out, false);
    Snapshot::from_bytes(out)
}

/// 1 回に変換するバイト数
const CHUNK: usize = 4 << 20;

fn add_stats(a: &mut DecodeStats, b: DecodeStats) {
    a.invalid += b.invalid;
    a.noncanonical += b.noncanonical;
    a.literal_escapes += b.literal_escapes;
    // 最後の区間の状態（区間は文書の順に加える）
    a.open_shift_at_end = b.open_shift_at_end;
}

/// `input` 全体をデコードして `w` に書く。`step(処理したバイト数)` が `false` を返したら中止して `None`。
///
/// LF の直後から独立にデコードできる文字コードでは、LF で区切った区間を並列にデコードする。
/// 書き込みは別スレッドで行い、デコードと並行させる。
pub(crate) fn decode_stream<W: Write + Send>(
    enc: Encoding,
    input: &[u8],
    escapes: bool,
    w: &mut W,
    step: &(dyn Fn(u64) -> bool + Sync),
) -> io::Result<Option<DecodeStats>> {
    std::thread::scope(|sc| {
        let (tx, rx) = bounded::<Vec<u8>>(8);
        let writer = sc.spawn(move || -> io::Result<()> {
            for buf in rx {
                w.write_all(&buf)?;
            }
            Ok(())
        });
        let stats = decode_segments(enc, input, escapes, &mut |b| tx.send(b).is_ok(), step);
        drop(tx);
        writer.join().expect("writer thread panicked")?;
        Ok(stats)
    })
}

/// デコードした内容を順に `emit` に渡す。`emit` か `step` が `false` を返したら中止して `None`。
fn decode_segments(
    enc: Encoding,
    input: &[u8],
    escapes: bool,
    emit: &mut dyn FnMut(Vec<u8>) -> bool,
    step: &(dyn Fn(u64) -> bool + Sync),
) -> Option<DecodeStats> {
    let threads = if enc.splits_at_lf() {
        std::thread::available_parallelism().map_or(1, |n| n.get().min(4))
    } else {
        1
    };
    let mut stats = DecodeStats::default();
    let mut pos = 0;
    while pos < input.len() {
        // 次の区間の終わり（CHUNK 以降の最初の LF の直後）
        let seg_end = |start: usize| -> Option<usize> {
            let from = (start + CHUNK).min(input.len());
            if from == input.len() {
                return Some(from);
            }
            let window = &input[from..(from + CHUNK).min(input.len())];
            memchr::memchr(b'\n', window).map(|k| from + k + 1)
        };
        let mut segs = Vec::new();
        let mut start = pos;
        if threads > 1 {
            while segs.len() < threads && start < input.len() {
                let Some(end) = seg_end(start) else { break };
                segs.push(start..end);
                start = end;
            }
        }
        if !segs.is_empty() {
            let results: Vec<(Vec<u8>, DecodeStats)> = if segs.len() == 1 {
                vec![yy_encoding::decode_all(
                    enc,
                    &input[segs[0].clone()],
                    escapes,
                )]
            } else {
                std::thread::scope(|sc| {
                    let hs: Vec<_> = segs
                        .iter()
                        .map(|r| {
                            let seg = &input[r.clone()];
                            sc.spawn(move || yy_encoding::decode_all(enc, seg, escapes))
                        })
                        .collect();
                    hs.into_iter().map(|h| h.join().unwrap()).collect()
                })
            };
            for (text, st) in results {
                if !emit(text) {
                    return None;
                }
                add_stats(&mut stats, st);
            }
            if !step((start - pos) as u64) {
                return None;
            }
            pos = start;
            continue;
        }
        // 1 つずつ: 次の LF（なければ最後）までを続けてデコードする
        let end = if threads > 1 {
            memchr::memchr(b'\n', &input[pos..]).map_or(input.len(), |k| pos + k + 1)
        } else {
            input.len()
        };
        let mut d = enc.new_decoder(escapes);
        let mut chunks = input[pos..end].chunks(CHUNK).peekable();
        while let Some(chunk) = chunks.next() {
            let mut out = Vec::with_capacity(chunk.len() * 3 / 2);
            d.decode(chunk, &mut out, chunks.peek().is_none());
            if !emit(out) || !step(chunk.len() as u64) {
                return None;
            }
        }
        add_stats(&mut stats, d.stats());
        pos = end;
    }
    Some(stats)
}

enum State {
    /// ジョブプールが渡されるのを待っている
    Pending,
    Running {
        job: JobHandle,
        rx: Receiver<io::Result<Decoded>>,
    },
}

/// 読み込む内容。
#[derive(Clone)]
enum Input {
    /// メモリマップしたファイルの範囲
    Mapped {
        source: Arc<MmapSource>,
        range: Range<usize>,
    },
    /// 書き込み共有が必要なファイル（作業用ファイルにコピーしてから、先頭 `bom_len` バイトを
    /// 除いて読む）
    Shared { path: PathBuf, bom_len: usize },
}

/// バックグラウンドの読み込み（コピーと変換）。ドロップするとキャンセルする。
pub(crate) struct Loader {
    input: Input,
    enc: Encoding,
    state: State,
}

impl Loader {
    pub fn new(source: Arc<MmapSource>, range: Range<usize>, enc: Encoding) -> Loader {
        Loader {
            input: Input::Mapped { source, range },
            enc,
            state: State::Pending,
        }
    }

    /// 書き込み共有が必要なファイルを作業用ファイルにコピーして読み込む（UTF-8 以外なら変換する）。
    pub fn shared(path: PathBuf, bom_len: usize, enc: Encoding) -> Loader {
        Loader {
            input: Input::Shared { path, bom_len },
            enc,
            state: State::Pending,
        }
    }

    /// まだ始めていなければ変換を始める。
    pub fn start(&mut self, pool: &JobPool, notify: Notifier) {
        if !matches!(self.state, State::Pending) {
            return;
        }
        let (tx, rx) = bounded(1);
        let (input, enc) = (self.input.clone(), self.enc);
        let job = pool.spawn(move |ctx| {
            let (source, range) = match input {
                Input::Mapped { source, range } => (source, range),
                Input::Shared { path, bom_len } => {
                    // 進捗は、コピーが終わると変換の分に切り替わる
                    ctx.progress
                        .set_total(std::fs::metadata(&path).map_or(0, |m| m.len()));
                    let copied = yy_io::copy_shared(&path, &mut |n| {
                        ctx.progress.set_done(n);
                        !ctx.cancel.is_cancelled()
                    });
                    let opened = match copied {
                        Err(e) if yy_io::is_cancelled(&e) => return,
                        Err(e) => {
                            let _ = tx.send(Err(e));
                            notify();
                            return;
                        }
                        Ok(o) => o,
                    };
                    let Some(source) = opened.source else {
                        let _ = tx.send(Ok(Decoded {
                            snapshot: Snapshot::empty(),
                            stats: DecodeStats::default(),
                            escapes: EscapeMode::Literal,
                        }));
                        notify();
                        return;
                    };
                    let len = source.bytes().len();
                    if enc == Encoding::Utf8 {
                        // UTF-8 はコピーをそのまま使う
                        let map: SourceRef = source;
                        let _ = tx.send(Ok(Decoded {
                            snapshot: Snapshot::from_source(
                                map,
                                bom_len.min(len) as u64..len as u64,
                                false,
                            ),
                            stats: DecodeStats::default(),
                            escapes: EscapeMode::Literal,
                        }));
                        notify();
                        return;
                    }
                    (source, bom_len.min(len)..len)
                }
            };
            let path = yy_io::temp_path("decode");
            let input = &source.bytes()[range];
            ctx.progress.set_total(input.len() as u64);
            let run = |escapes: bool| -> io::Result<Option<DecodeStats>> {
                let file = OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .open(&path)?;
                let mut w = BufWriter::with_capacity(1 << 20, file);
                ctx.progress.set_done(0);
                let stats = decode_stream(enc, input, escapes, &mut w, &|n| {
                    ctx.progress.add_done(n);
                    !ctx.cancel.is_cancelled()
                })?;
                // 一時ファイルなので永続化（sync）は不要
                w.flush()?;
                Ok(stats)
            };
            let result = (|| -> io::Result<Option<Decoded>> {
                let Some(mut stats) = run(true)? else {
                    return Ok(None);
                };
                let mut escapes = EscapeMode::Restore;
                if stats.literal_escapes > 0 {
                    let Some(s) = run(false)? else {
                        return Ok(None);
                    };
                    stats = s;
                    escapes = EscapeMode::Literal;
                }
                let len = std::fs::metadata(&path)?.len();
                let snapshot = if len == 0 {
                    let _ = std::fs::remove_file(&path);
                    Snapshot::empty()
                } else {
                    let map: SourceRef = yy_io::map_temp(&path)?;
                    Snapshot::from_source(map, 0..len, false)
                };
                Ok(Some(Decoded {
                    snapshot,
                    stats,
                    escapes,
                }))
            })();
            let msg = match result {
                Ok(None) => {
                    let _ = std::fs::remove_file(&path);
                    return;
                }
                Ok(Some(d)) => Ok(d),
                Err(e) => {
                    let _ = std::fs::remove_file(&path);
                    Err(e)
                }
            };
            let _ = tx.send(msg);
            notify();
        });
        self.state = State::Running { job, rx };
    }

    /// 終わっていれば結果を返す。
    pub fn poll(&mut self) -> Option<io::Result<Decoded>> {
        match &self.state {
            State::Running { rx, .. } => rx.try_recv().ok(),
            State::Pending => None,
        }
    }

    pub fn progress(&self) -> f64 {
        match &self.state {
            State::Running { job, .. } => job.progress().fraction(),
            State::Pending => 0.0,
        }
    }
}

impl Drop for Loader {
    fn drop(&mut self) {
        if let State::Running { job, .. } = &self.state {
            job.cancel();
        }
    }
}
