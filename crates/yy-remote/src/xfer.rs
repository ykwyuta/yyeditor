//! 巨大なファイルの転送（SFTP・SCP）と、切断後のレジューム（13 章 5）。
//!
//! 転送の 1 件（[`Job`]）は、送り終えたことが確かな位置（`done`）をジャーナル（[`Journal`]）に
//! 書きながら進める。接続が切れたら（[`retryable`]）、間を空けて接続し直し、ジャーナルの位置と
//! 途中のファイル（`<名前>.yypart`）の大きさの小さい方から続ける。SFTP のアップロードでは、
//! 続ける位置の直前の 64 KiB を接続先から読んで手元と比べ、違えば最初からにする。
//! 書き終えたら大きさを確かめてから本来の名前に変える（途中のファイルが本物と取り違えられない）。
//!
//! 転送の様子（接続、続ける位置の決め方、進み・速さ、切断と再接続、確認、名前の変更）は
//! [`TransferLog`] に細かく書く。アプリを閉じても、ジャーナルから再開できる。

use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::log::TransferLog;
use crate::scp;
use crate::sftp::{self, Attrs, Sftp};
use crate::uri::RemoteUri;
use crate::{Transport, shell_quote};

/// 転送の方式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Protocol {
    Sftp,
    Scp,
}

impl Protocol {
    pub fn name(self) -> &'static str {
        match self {
            Protocol::Sftp => "SFTP",
            Protocol::Scp => "SCP",
        }
    }
}

/// 転送の向き。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// 手元 → 接続先
    Upload,
    /// 接続先 → 手元
    Download,
}

/// 転送の状態。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Queued,
    Running,
    /// 一時停止・切断（再開できる）
    Interrupted,
    Done,
    Failed,
}

impl State {
    pub fn label(self) -> &'static str {
        match self {
            State::Queued => "待機中",
            State::Running => "転送中",
            State::Interrupted => "中断（再開できます）",
            State::Done => "完了",
            State::Failed => "失敗",
        }
    }
}

/// 転送の 1 件（1 つのファイル）。
#[derive(Clone, Debug)]
pub struct Job {
    pub id: u64,
    pub direction: Direction,
    pub protocol: Protocol,
    /// 手元のファイル
    pub local: PathBuf,
    /// 接続先のファイル
    pub remote: RemoteUri,
    /// 送るファイルの大きさと更新日時（送る側のもの。変わっていたら続けない）
    pub size: u64,
    pub mtime: u64,
    /// 送り終えたことが確かな量（ジャーナルに書く）
    pub done: u64,
    /// 送っている途中の量（表示用。ジャーナルには書かない）
    pub current: u64,
    pub state: State,
    /// 失敗・中断の理由
    pub message: String,
    /// 同じ名前のファイルがあれば置き換える
    pub overwrite: bool,
}

impl Job {
    /// 途中のファイルの接続先のパス。
    pub fn remote_part(&self) -> Vec<u8> {
        let mut p = self.remote.path.clone();
        p.extend_from_slice(b".yypart");
        p
    }

    /// 途中のファイルの手元のパス。
    pub fn local_part(&self) -> PathBuf {
        let mut s = self.local.as_os_str().to_owned();
        s.push(".yypart");
        PathBuf::from(s)
    }

    /// `手元 → 接続先` の表示。
    pub fn label(&self) -> String {
        match self.direction {
            Direction::Upload => format!("{} → {}", self.local.display(), self.remote),
            Direction::Download => format!("{} → {}", self.remote, self.local.display()),
        }
    }
}

/// 手元のファイルの更新日時（秒）。
pub fn mtime_of(meta: &std::fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs())
}

/// 大きさの表示（`1.50 GiB`）。
pub fn human(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i + 1 < UNITS.len() {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else {
        format!("{v:.2} {}", UNITS[i])
    }
}

// ---- ジャーナル -----------------------------------------------------------------

/// 転送のジャーナル（フォルダの中に 1 件 1 ファイル。書き換えは一時ファイルから置き換える）。
pub struct Journal {
    dir: PathBuf,
}

fn escape(s: &str) -> String {
    s.replace('%', "%25")
        .replace('\n', "%0A")
        .replace('\r', "%0D")
}

fn unescape(s: &str) -> String {
    s.replace("%0A", "\n")
        .replace("%0D", "\r")
        .replace("%25", "%")
}

impl Journal {
    pub fn new(dir: impl Into<PathBuf>) -> Journal {
        Journal { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn file(&self, id: u64) -> PathBuf {
        self.dir.join(format!("{id}.job"))
    }

    /// 1 件を書く（完了したものは消す）。
    pub fn save(&self, job: &Job) -> io::Result<()> {
        if job.state == State::Done {
            return self.remove(job.id);
        }
        std::fs::create_dir_all(&self.dir)?;
        let state = match job.state {
            // 動いていた転送は、読み直したときには中断とみなす
            State::Running | State::Interrupted => "interrupted",
            State::Queued => "queued",
            State::Failed => "failed",
            State::Done => "done",
        };
        let text = format!(
            "direction={}\nprotocol={}\nlocal={}\nremote={}\nsize={}\nmtime={}\ndone={}\nstate={state}\noverwrite={}\nmessage={}\n",
            match job.direction {
                Direction::Upload => "upload",
                Direction::Download => "download",
            },
            match job.protocol {
                Protocol::Sftp => "sftp",
                Protocol::Scp => "scp",
            },
            escape(&job.local.to_string_lossy()),
            escape(&job.remote.to_string()),
            job.size,
            job.mtime,
            job.done,
            job.overwrite,
            escape(&job.message),
        );
        let tmp = self.dir.join(format!("{}.tmp", job.id));
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, self.file(job.id))
    }

    pub fn remove(&self, id: u64) -> io::Result<()> {
        match std::fs::remove_file(self.file(id)) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }

    /// すべての件（番号順）。読めないファイルは飛ばす。
    pub fn load(&self) -> Vec<Job> {
        let Ok(rd) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut jobs: Vec<Job> = rd
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                let id: u64 = name.strip_suffix(".job")?.parse().ok()?;
                let text = std::fs::read_to_string(e.path()).ok()?;
                parse_job(id, &text)
            })
            .collect();
        jobs.sort_by_key(|j| j.id);
        jobs
    }

    /// 次の番号。
    pub fn next_id(&self) -> u64 {
        self.load().iter().map(|j| j.id).max().unwrap_or(0) + 1
    }
}

fn parse_job(id: u64, text: &str) -> Option<Job> {
    let get = |k: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(k)?.strip_prefix('='))
            .map(unescape)
    };
    Some(Job {
        id,
        direction: match get("direction")?.as_str() {
            "upload" => Direction::Upload,
            "download" => Direction::Download,
            _ => return None,
        },
        protocol: match get("protocol")?.as_str() {
            "scp" => Protocol::Scp,
            _ => Protocol::Sftp,
        },
        local: PathBuf::from(get("local")?),
        remote: RemoteUri::parse(&get("remote")?)?,
        size: get("size")?.parse().ok()?,
        mtime: get("mtime")?.parse().ok()?,
        done: get("done")?.parse().ok()?,
        current: 0,
        state: match get("state")?.as_str() {
            "queued" => State::Queued,
            "failed" => State::Failed,
            _ => State::Interrupted,
        },
        message: get("message").unwrap_or_default(),
        overwrite: get("overwrite").is_some_and(|v| v == "true"),
    })
    .map(|mut j| {
        j.current = j.done;
        j
    })
}

// ---- 実行 -----------------------------------------------------------------------

/// 接続する関数（再接続にも使う。記録は転送の記録に写す）。
pub type Connect<'a> =
    dyn Fn(&TransferLog, u64) -> io::Result<Arc<dyn Transport>> + Send + Sync + 'a;

/// 再接続の決まり。
#[derive(Clone, Debug)]
pub struct Retry {
    /// 進みがないまま続けて失敗してよい回数
    pub attempts: u32,
    /// 接続し直す前の待ち時間（回数ごと。足りなければ最後のもの）
    pub delays: Vec<Duration>,
}

impl Default for Retry {
    fn default() -> Self {
        Retry {
            attempts: 10,
            delays: [2, 5, 10, 20, 30, 60]
                .into_iter()
                .map(Duration::from_secs)
                .collect(),
        }
    }
}

/// 転送の実行に使うもの。
pub struct Context<'a> {
    pub connect: &'a Connect<'a>,
    pub log: &'a TransferLog,
    pub journal: Option<&'a Journal>,
    /// 立てると一時停止する（再開できる）
    pub cancel: &'a AtomicBool,
    /// 進みの知らせ（`job.current`・`job.state` を見る。多くても 0.2 秒に 1 回）
    pub progress: &'a mut dyn FnMut(&Job),
    pub retry: Retry,
    /// SCP のアップロードで 1 回に送る量（切断で失うのは多くてもこの量）
    pub scp_chunk: u64,
    /// 最初に使う接続（なければ接続する）
    pub transport: Option<Arc<dyn Transport>>,
}

impl Context<'_> {
    fn note(&self, job: &Job, msg: &str) {
        self.log.line(Some(job.id), msg);
    }

    fn save(&self, job: &Job) {
        if let Some(j) = self.journal
            && let Err(e) = j.save(job)
        {
            self.log
                .line(Some(job.id), &format!("ジャーナルを書けません: {e}"));
        }
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

/// 再接続して続けられる失敗か（接続が切れた）。
pub fn retryable(e: &io::Error, t: Option<&dyn Transport>) -> bool {
    use io::ErrorKind::*;
    matches!(
        e.kind(),
        ConnectionAborted
            | ConnectionReset
            | BrokenPipe
            | UnexpectedEof
            | TimedOut
            | NotConnected
            | ConnectionRefused
            | HostUnreachable
            | NetworkUnreachable
    ) || t.is_some_and(|t| t.is_closed())
}

fn interrupted() -> io::Error {
    io::Error::new(io::ErrorKind::Interrupted, "一時停止しました")
}

/// 進みの記録・知らせ・ジャーナルの書き込みを間引く。
struct Meter {
    start: Instant,
    start_bytes: u64,
    last_note: Instant,
    last_note_bytes: u64,
    last_save: Instant,
    last_progress: Instant,
}

impl Meter {
    fn new(offset: u64) -> Meter {
        let now = Instant::now();
        Meter {
            start: now,
            start_bytes: offset,
            last_note: now,
            last_note_bytes: offset,
            last_save: now,
            last_progress: now - Duration::from_secs(1),
        }
    }

    /// 進んだ（`job.current` は更新済み。`confirmed` なら `job.done` も進んだ）。
    fn tick(&mut self, job: &Job, cx: &mut Context, confirmed: bool) {
        let now = Instant::now();
        if now - self.last_progress >= Duration::from_millis(200) {
            self.last_progress = now;
            (cx.progress)(job);
        }
        if confirmed && now - self.last_save >= Duration::from_secs(1) {
            self.last_save = now;
            cx.save(job);
        }
        let moved = job.current.saturating_sub(self.last_note_bytes);
        if now - self.last_note >= Duration::from_secs(10) || moved >= 256 << 20 {
            let secs = (now - self.last_note).as_secs_f64().max(0.001);
            let pct = if job.size == 0 {
                100.0
            } else {
                job.current as f64 * 100.0 / job.size as f64
            };
            cx.note(
                job,
                &format!(
                    "進み {} / {}（{pct:.1}%）、{}/秒、確定 {}",
                    human(job.current),
                    human(job.size),
                    human((moved as f64 / secs) as u64),
                    human(job.done)
                ),
            );
            self.last_note = now;
            self.last_note_bytes = job.current;
        }
    }

    fn average(&self, end: u64) -> String {
        let secs = self.start.elapsed().as_secs_f64().max(0.001);
        format!(
            "{} を {:.1} 秒（{}/秒）",
            human(end.saturating_sub(self.start_bytes)),
            secs,
            human((end.saturating_sub(self.start_bytes) as f64 / secs) as u64)
        )
    }
}

/// 1 件を最後まで（または中断・失敗まで）転送する。`job.state` が結果。
pub fn run(job: &mut Job, cx: &mut Context) {
    job.state = State::Running;
    job.message.clear();
    job.current = job.done;
    cx.save(job);
    (cx.progress)(job);
    cx.note(
        job,
        &format!(
            "開始: {} {}（{}、{}）{}",
            job.protocol.name(),
            match job.direction {
                Direction::Upload => "アップロード",
                Direction::Download => "ダウンロード",
            },
            job.label(),
            human(job.size),
            if job.done > 0 {
                format!("。ジャーナルでは {} まで送り終えています", human(job.done))
            } else {
                String::new()
            }
        ),
    );
    let mut transport = cx.transport.take();
    let mut failures = 0u32;
    loop {
        if cx.cancelled() {
            return finish_interrupted(job, cx, "一時停止しました");
        }
        let t = match transport.clone().filter(|t| !t.is_closed()) {
            Some(t) => t,
            None => {
                cx.note(job, &format!("{} に接続します", job.remote.target()));
                match (cx.connect)(cx.log, job.id) {
                    Ok(t) => {
                        cx.note(job, "接続しました");
                        t
                    }
                    Err(e) => {
                        // 認証の中止・失敗は待っても直らない
                        if matches!(
                            e.kind(),
                            io::ErrorKind::Interrupted | io::ErrorKind::PermissionDenied
                        ) || cx.cancelled()
                        {
                            return finish_interrupted(
                                job,
                                cx,
                                &format!("接続できませんでした: {e}"),
                            );
                        }
                        failures += 1;
                        if !wait_retry(job, cx, failures, &format!("接続できませんでした: {e}"))
                        {
                            return;
                        }
                        continue;
                    }
                }
            }
        };
        let before = job.done;
        let r = match (job.direction, job.protocol) {
            (Direction::Upload, Protocol::Sftp) => upload_sftp(job, &t, cx),
            (Direction::Download, Protocol::Sftp) => download_sftp(job, &t, cx),
            (Direction::Upload, Protocol::Scp) => upload_scp(job, &t, cx),
            (Direction::Download, Protocol::Scp) => download_scp(job, &t, cx),
        };
        match r {
            Ok(()) => {
                // 次の転送でも同じ接続を使う
                cx.transport = Some(t);
                job.state = State::Done;
                job.done = job.size;
                job.current = job.size;
                cx.save(job);
                cx.note(job, "完了しました");
                (cx.progress)(job);
                return;
            }
            Err(_) if cx.cancelled() => {
                return finish_interrupted(job, cx, "一時停止しました");
            }
            Err(e) if retryable(&e, Some(t.as_ref())) => {
                transport = None;
                if job.done > before {
                    // 進みがあれば数え直す（切断が時々ある長い転送も続ける）
                    failures = 0;
                }
                failures += 1;
                job.current = job.done;
                cx.save(job);
                if !wait_retry(job, cx, failures, &format!("接続が切れました: {e}")) {
                    return;
                }
            }
            Err(e) => {
                job.state = State::Failed;
                job.message = e.to_string();
                job.current = job.done;
                cx.save(job);
                cx.note(job, &format!("失敗しました: {e}"));
                (cx.progress)(job);
                return;
            }
        }
    }
}

fn finish_interrupted(job: &mut Job, cx: &mut Context, why: &str) {
    job.state = State::Interrupted;
    job.message = why.to_owned();
    job.current = job.done;
    cx.save(job);
    cx.note(
        job,
        &format!(
            "{why}。{} まで送り終えています（再開できます）",
            human(job.done)
        ),
    );
    (cx.progress)(job);
}

/// 再接続の前に待つ。あきらめたら `false`（中断にする）。
fn wait_retry(job: &mut Job, cx: &mut Context, failures: u32, why: &str) -> bool {
    if failures > cx.retry.attempts {
        finish_interrupted(
            job,
            cx,
            &format!("{why}。{} 回続けて失敗したので中断しました", failures - 1),
        );
        return false;
    }
    let delay = cx
        .retry
        .delays
        .get(failures as usize - 1)
        .or(cx.retry.delays.last())
        .copied()
        .unwrap_or_default();
    job.message = format!("再接続を待っています（{} 回目）", failures);
    cx.note(
        job,
        &format!(
            "{why}。{:.0} 秒後に再接続します（{failures}/{} 回目）",
            delay.as_secs_f64(),
            cx.retry.attempts
        ),
    );
    (cx.progress)(job);
    let until = Instant::now() + delay;
    while Instant::now() < until {
        if cx.cancelled() {
            finish_interrupted(job, cx, "一時停止しました");
            return false;
        }
        std::thread::sleep(Duration::from_millis(50).min(until - Instant::now()));
    }
    true
}

/// 手元のファイルが送り始めたときと同じか確かめる（変わっていたら続けない）。
fn check_local(job: &Job) -> io::Result<File> {
    let f = File::open(&job.local)
        .map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", job.local.display())))?;
    let meta = f.metadata()?;
    if meta.len() != job.size || mtime_of(&meta) != job.mtime {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{} が送り始めてから変更されました（大きさ {} → {}）。もう一度送り直してください",
                job.local.display(),
                job.size,
                meta.len()
            ),
        ));
    }
    Ok(f)
}

/// `buf` が埋まるか EOF まで読む。
fn read_full(r: &mut dyn Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}

/// 接続先のフォルダを（親から順に）作る。
fn mkdir_all(s: &Sftp, dir: &[u8]) -> io::Result<()> {
    if dir.is_empty() || dir == b"/" || s.try_stat(dir)?.is_some() {
        return Ok(());
    }
    if let Some(i) = dir.iter().rposition(|&b| b == b'/') {
        mkdir_all(s, &dir[..i])?;
    }
    match s.mkdir(dir) {
        Err(e) if s.try_stat(dir)?.is_none() => Err(e),
        _ => Ok(()),
    }
}

fn parent(path: &[u8]) -> &[u8] {
    match path.iter().rposition(|&b| b == b'/') {
        Some(0) => b"/",
        Some(i) => &path[..i],
        None => b"",
    }
}

/// SFTP で読む（短く返ってきたら続きを読む）。
fn read_exact_at(
    s: &Sftp,
    h: &sftp::Handle,
    mut offset: u64,
    mut len: usize,
) -> io::Result<Vec<u8>> {
    let mut out = Vec::with_capacity(len);
    while len > 0 {
        let d = s
            .send_read(h, offset, (len as u32).min(sftp::CHUNK))?
            .wait()?;
        if d.is_empty() {
            break;
        }
        offset += d.len() as u64;
        len -= d.len();
        out.extend_from_slice(&d);
    }
    Ok(out)
}

/// 照合に使う、続ける位置の直前の量
const VERIFY_TAIL: u64 = 64 << 10;

fn upload_sftp(job: &mut Job, t: &Arc<dyn Transport>, cx: &mut Context) -> io::Result<()> {
    let mut local = check_local(job)?;
    let s = Sftp::connect(t.as_ref())?;
    let ext: Vec<&str> = s.extensions().iter().map(|(n, _)| n.as_str()).collect();
    cx.note(
        job,
        &format!("SFTP 版 {}、拡張: {}", s.version(), ext.join(", ")),
    );
    let target = job.remote.path.clone();
    let part = job.remote_part();
    mkdir_all(&s, parent(&target))?;
    if job.done == 0
        && !job.overwrite
        && let Some(a) = s.try_stat(&target)?
    {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!(
                "接続先に {} が既にあります（{}）",
                crate::display(&target),
                human(a.size.unwrap_or(0))
            ),
        ));
    }
    // 続ける位置
    let mut offset = 0;
    if job.done > 0 {
        match s.try_stat(&part)? {
            None => cx.note(job, "途中のファイルが接続先にないので、最初から送ります"),
            Some(a) => {
                let remote = a.size.unwrap_or(0);
                offset = job.done.min(remote);
                cx.note(
                    job,
                    &format!(
                        "レジューム: ジャーナル {}、接続先の途中のファイル {} → {} から続けます",
                        human(job.done),
                        human(remote),
                        human(offset)
                    ),
                );
                if offset > 0 {
                    // 直前の部分が手元と同じか確かめる
                    let n = VERIFY_TAIL.min(offset);
                    let h = s.open(&part, sftp::open::READ, &Attrs::default())?;
                    let theirs = read_exact_at(&s, &h, offset - n, n as usize);
                    let _ = s.close(&h);
                    let theirs = theirs?;
                    let mut ours = vec![0u8; n as usize];
                    local.seek(SeekFrom::Start(offset - n))?;
                    local.read_exact(&mut ours)?;
                    if theirs == ours {
                        cx.note(
                            job,
                            &format!("照合: 続ける位置の直前の {} が一致しました", human(n)),
                        );
                    } else {
                        cx.note(
                            job,
                            "照合: 続ける位置の直前が手元と違うため、最初から送ります",
                        );
                        offset = 0;
                    }
                }
            }
        }
    }
    job.done = offset;
    job.current = offset;
    let flags =
        sftp::open::WRITE | sftp::open::CREATE | if offset == 0 { sftp::open::TRUNCATE } else { 0 };
    let h = s.open(
        &part,
        flags,
        &Attrs {
            permissions: Some(0o644),
            ..Attrs::default()
        },
    )?;
    cx.note(
        job,
        &format!(
            "{} を開きました。{} から送ります（要求 {} KiB × {} 件を並べる）",
            crate::display(&part),
            human(offset),
            sftp::CHUNK / 1024,
            sftp::WINDOW
        ),
    );
    local.seek(SeekFrom::Start(offset))?;
    let mut meter = Meter::new(offset);
    let mut inflight: VecDeque<(u64, sftp::WriteReply)> = VecDeque::new();
    let mut pos = offset;
    let mut buf = vec![0u8; sftp::CHUNK as usize];
    let result = (|| {
        while pos < job.size || !inflight.is_empty() {
            while inflight.len() < sftp::WINDOW && pos < job.size {
                let want = (sftp::CHUNK as u64).min(job.size - pos) as usize;
                let n = read_full(&mut local, &mut buf[..want])?;
                if n < want {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("{} が送っている間に短くなりました", job.local.display()),
                    ));
                }
                let w = s.send_write(&h, pos, &buf[..n])?;
                pos += n as u64;
                inflight.push_back((pos, w));
                job.current = pos;
            }
            let (end, w) = inflight.pop_front().expect("non-empty");
            w.wait()?;
            // 先頭から順に確かめるので、ここまでは途切れなく書けている
            job.done = end;
            meter.tick(job, cx, true);
            if cx.cancelled() {
                return Err(interrupted());
            }
        }
        Ok(())
    })();
    if let Err(e) = result {
        // 切れる前に届いていた応答の分は、書けたことが確かなので数える
        let before = job.done;
        while let Some((end, w)) = inflight.pop_front() {
            if w.wait().is_err() {
                break;
            }
            job.done = end;
        }
        if job.done > before {
            cx.note(
                job,
                &format!(
                    "切れる前に届いていた応答で、{} まで確定しました",
                    human(job.done)
                ),
            );
        }
        job.current = job.done;
        let _ = s.close(&h);
        return Err(e);
    }
    let synced = s.fsync(&h).unwrap_or(false);
    s.close(&h)?;
    cx.note(
        job,
        &format!(
            "送り終えました: {}{}",
            meter.average(job.size),
            if synced { "（fsync 済み）" } else { "" }
        ),
    );
    let got = s.stat(&part)?.size.unwrap_or(0);
    if got != job.size {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("接続先の大きさが違います（{} / {}）", got, job.size),
        ));
    }
    cx.note(
        job,
        &format!("確認: 接続先の大きさ {got} バイトが一致しました"),
    );
    s.rename(&part, &target, true)?;
    cx.note(
        job,
        &format!(
            "{} を {} に名前を変えました",
            crate::display(&part),
            crate::display(&target)
        ),
    );
    let t32 = u32::try_from(job.mtime).unwrap_or(u32::MAX);
    if let Err(e) = s.setstat(
        &target,
        &Attrs {
            atime: Some(t32),
            mtime: Some(t32),
            ..Attrs::default()
        },
    ) {
        cx.note(job, &format!("更新日時を設定できませんでした: {e}"));
    }
    Ok(())
}

/// 書き終えた手元の途中のファイルを本来の名前にする。
fn finish_local(job: &Job, cx: &Context, file: File) -> io::Result<()> {
    file.sync_all()?;
    let len = file.metadata()?.len();
    if len != job.size {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("受け取った大きさが違います（{len} / {}）", job.size),
        ));
    }
    if job.mtime > 0 {
        let _ = file.set_modified(UNIX_EPOCH + Duration::from_secs(job.mtime));
    }
    drop(file);
    cx.note(
        job,
        &format!("確認: 手元の大きさ {len} バイトが一致しました"),
    );
    let part = job.local_part();
    if job.overwrite && job.local.exists() {
        std::fs::remove_file(&job.local)?;
    }
    std::fs::rename(&part, &job.local)?;
    cx.note(
        job,
        &format!(
            "{} を {} に名前を変えました",
            part.display(),
            job.local.display()
        ),
    );
    Ok(())
}

/// 手元の途中のファイルを `offset` の位置で開く（それより後ろは捨てる）。
fn open_local_part(job: &mut Job, cx: &Context) -> io::Result<(File, u64)> {
    if let Some(dir) = job.local.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if job.done == 0 && !job.overwrite && job.local.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("手元に {} が既にあります", job.local.display()),
        ));
    }
    let part = job.local_part();
    let have = std::fs::metadata(&part).map_or(0, |m| m.len());
    let offset = if job.done > 0 {
        let o = job.done.min(have);
        cx.note(
            job,
            &format!(
                "レジューム: ジャーナル {}、手元の途中のファイル {} → {} から続けます",
                human(job.done),
                human(have),
                human(o)
            ),
        );
        o
    } else {
        0
    };
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&part)?;
    f.set_len(offset)?;
    f.seek(SeekFrom::Start(offset))?;
    job.done = offset;
    job.current = offset;
    Ok((f, offset))
}

fn download_sftp(job: &mut Job, t: &Arc<dyn Transport>, cx: &mut Context) -> io::Result<()> {
    let s = Sftp::connect(t.as_ref())?;
    cx.note(job, &format!("SFTP 版 {}", s.version()));
    let a = s.stat(&job.remote.path)?;
    let size = a.size.unwrap_or(0);
    let mtime = u64::from(a.mtime.unwrap_or(0));
    if size != job.size || mtime != job.mtime {
        if job.done > 0 {
            cx.note(
                job,
                &format!(
                    "接続先のファイルが変わったため（{} → {}）、最初から受け取ります",
                    human(job.size),
                    human(size)
                ),
            );
        }
        job.size = size;
        job.mtime = mtime;
        job.done = 0;
    }
    let (mut file, offset) = open_local_part(job, cx)?;
    let h = s.open(&job.remote.path, sftp::open::READ, &Attrs::default())?;
    cx.note(
        job,
        &format!(
            "{} を開きました。{} から受け取ります",
            job.remote,
            human(offset)
        ),
    );
    let mut meter = Meter::new(offset);
    let mut inflight: VecDeque<(u64, u32, sftp::ReadReply)> = VecDeque::new();
    let mut pos = offset;
    let result = (|| {
        while job.done < job.size {
            while inflight.len() < sftp::WINDOW && pos < job.size {
                let len = (sftp::CHUNK as u64).min(job.size - pos) as u32;
                inflight.push_back((pos, len, s.send_read(&h, pos, len)?));
                pos += u64::from(len);
            }
            let Some((at, len, r)) = inflight.pop_front() else {
                break;
            };
            let mut data = r.wait()?;
            if data.len() < len as usize {
                // 短く返ってきた: 続きをこの場で読む
                let rest =
                    read_exact_at(&s, &h, at + data.len() as u64, len as usize - data.len())?;
                data.extend_from_slice(&rest);
                if data.len() < len as usize {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "接続先のファイルが受け取っている間に短くなりました",
                    ));
                }
            }
            file.write_all(&data)?;
            job.done = at + data.len() as u64;
            job.current = job.done;
            meter.tick(job, cx, true);
            if cx.cancelled() {
                return Err(interrupted());
            }
        }
        Ok(())
    })();
    let _ = s.close(&h);
    result?;
    cx.note(
        job,
        &format!("受け取り終えました: {}", meter.average(job.size)),
    );
    finish_local(job, cx, file)
}

fn upload_scp(job: &mut Job, t: &Arc<dyn Transport>, cx: &mut Context) -> io::Result<()> {
    let mut local = check_local(job)?;
    let target = job.remote.path.clone();
    let part = job.remote_part();
    let mut chunk_path = part.clone();
    chunk_path.extend_from_slice(b".chunk");
    let mut mkdir = b"mkdir -p ".to_vec();
    mkdir.extend_from_slice(&shell_quote(parent(&target)));
    scp::shell(t.as_ref(), &mkdir, "フォルダの作成")?;
    let mut offset = 0;
    if job.done > 0 {
        let have = scp::remote_size(t.as_ref(), &part)?.unwrap_or(0);
        // SCP では区切りを順につなげるので、接続先の途中のファイルの中身は手元の先頭と同じ
        offset = have.min(job.size);
        cx.note(
            job,
            &format!(
                "レジューム: ジャーナル {}、接続先の途中のファイル {} → {} から続けます",
                human(job.done),
                human(have),
                human(offset)
            ),
        );
    } else if !job.overwrite && scp::remote_size(t.as_ref(), &target)?.is_some() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("接続先に {} が既にあります", crate::display(&target)),
        ));
    }
    job.done = offset;
    job.current = offset;
    let mut meter = Meter::new(offset);
    while offset < job.size {
        let n = cx.scp_chunk.max(1).min(job.size - offset);
        local.seek(SeekFrom::Start(offset))?;
        let mut reader = (&mut local).take(n);
        cx.note(
            job,
            &format!(
                "区切りを送ります: {}〜{}（{}）",
                human(offset),
                human(offset + n),
                human(n)
            ),
        );
        let base = offset;
        let cancel = cx.cancel;
        let r = {
            let mut progress = |sent: u64| {
                job.current = base + sent;
                meter.tick(job, cx, false);
                !cancel.load(Ordering::Relaxed)
            };
            scp::upload(
                t.as_ref(),
                &chunk_path,
                0o600,
                n,
                &mut reader,
                &mut progress,
            )
        };
        if let Err(e) = r {
            return Err(if cx.cancelled() { interrupted() } else { e });
        }
        // 途中のファイルにつなげる（最初の区切りはそのまま途中のファイルにする）
        let mut cmd = Vec::new();
        if offset == 0 {
            cmd.extend_from_slice(b"mv -f ");
            cmd.extend_from_slice(&shell_quote(&chunk_path));
            cmd.push(b' ');
            cmd.extend_from_slice(&shell_quote(&part));
        } else {
            cmd.extend_from_slice(b"cat ");
            cmd.extend_from_slice(&shell_quote(&chunk_path));
            cmd.extend_from_slice(b" >> ");
            cmd.extend_from_slice(&shell_quote(&part));
            cmd.extend_from_slice(b" && rm -f ");
            cmd.extend_from_slice(&shell_quote(&chunk_path));
        }
        scp::shell(t.as_ref(), &cmd, "途中のファイルへの追記")?;
        offset += n;
        job.done = offset;
        job.current = offset;
        cx.save(job);
        meter.tick(job, cx, true);
        if cx.cancelled() {
            return Err(interrupted());
        }
    }
    cx.note(job, &format!("送り終えました: {}", meter.average(job.size)));
    let got = scp::remote_size(t.as_ref(), &part)?.unwrap_or(0);
    if got != job.size {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("接続先の大きさが違います（{got} / {}）", job.size),
        ));
    }
    cx.note(
        job,
        &format!("確認: 接続先の大きさ {got} バイトが一致しました"),
    );
    let mut cmd = b"mv -f ".to_vec();
    cmd.extend_from_slice(&shell_quote(&part));
    cmd.push(b' ');
    cmd.extend_from_slice(&shell_quote(&target));
    cmd.extend_from_slice(format!(" && (touch -m -d @{} ", job.mtime).as_bytes());
    cmd.extend_from_slice(&shell_quote(&target));
    cmd.extend_from_slice(b" 2>/dev/null || true)");
    scp::shell(t.as_ref(), &cmd, "名前の変更")?;
    cx.note(
        job,
        &format!(
            "{} を {} に名前を変えました",
            crate::display(&part),
            crate::display(&target)
        ),
    );
    Ok(())
}

fn download_scp(job: &mut Job, t: &Arc<dyn Transport>, cx: &mut Context) -> io::Result<()> {
    let size = scp::remote_size(t.as_ref(), &job.remote.path)?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("接続先に {} がありません", job.remote),
        )
    })?;
    if size != job.size {
        if job.done > 0 {
            cx.note(
                job,
                "接続先のファイルの大きさが変わったため、最初から受け取ります",
            );
        }
        job.size = size;
        job.done = 0;
    }
    let (mut file, offset) = open_local_part(job, cx)?;
    cx.note(
        job,
        &format!(
            "{} から受け取ります（{}）",
            human(offset),
            if offset == 0 {
                "scp -f"
            } else {
                "途中からは tail -c +N"
            }
        ),
    );
    let mut meter = Meter::new(offset);
    let cancel = cx.cancel;
    let remote = job.remote.path.clone();
    let r = {
        let mut progress = |got: u64| {
            job.current = offset + got;
            // 書いた分は手元にある（受け取った順に書くので途切れない）
            job.done = job.current;
            meter.tick(job, cx, true);
            !cancel.load(Ordering::Relaxed)
        };
        scp::download(t.as_ref(), &remote, offset, &mut file, &mut progress)
    };
    if let Err(e) = r {
        let _ = file.flush();
        job.done = file.stream_position().unwrap_or(job.done);
        return Err(if cx.cancelled() { interrupted() } else { e });
    }
    job.done = file.stream_position()?;
    if job.done != job.size {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            format!("途中で切れました（{} / {}）", job.done, job.size),
        ));
    }
    cx.note(
        job,
        &format!("受け取り終えました: {}", meter.average(job.size)),
    );
    finish_local(job, cx, file)
}

/// 送るファイルの大きさと更新日時を、ジョブに書く（アップロード）。
pub fn upload_job(
    id: u64,
    protocol: Protocol,
    local: &Path,
    remote: RemoteUri,
    overwrite: bool,
) -> io::Result<Job> {
    let meta = std::fs::metadata(local)?;
    Ok(Job {
        id,
        direction: Direction::Upload,
        protocol,
        local: local.to_owned(),
        remote,
        size: meta.len(),
        mtime: mtime_of(&meta),
        done: 0,
        current: 0,
        state: State::Queued,
        message: String::new(),
        overwrite,
    })
}

/// ダウンロードのジョブ（大きさ・更新日時は接続先の一覧のもの。始めるときに確かめ直す）。
pub fn download_job(
    id: u64,
    protocol: Protocol,
    remote: RemoteUri,
    size: u64,
    mtime: u64,
    local: &Path,
    overwrite: bool,
) -> Job {
    Job {
        id,
        direction: Direction::Download,
        protocol,
        local: local.to_owned(),
        remote,
        size,
        mtime,
        done: 0,
        current: 0,
        state: State::Queued,
        message: String::new(),
        overwrite,
    }
}

/// 今の時刻（秒）。
pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(all(test, unix))]
mod tests;
