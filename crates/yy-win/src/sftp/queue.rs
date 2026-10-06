//! 転送の一覧と記録（yysftp の下の欄。13 章 7）。
//!
//! 転送は 1 本の作業スレッドが順に [`xfer::run`] で行う（切断されたら再接続して続きから送る）。
//! 進みと記録の行はチャネルで UI に送り、フレームには溜まっている間 1 回だけ知らせる。
//! 一時停止・中断した転送はジャーナル（設定のフォルダの `transfers`）に残り、次に起動したときにも
//! 続きから再開できる。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateFontW, DEFAULT_CHARSET, DeleteObject, FF_MODERN,
    FIXED_PITCH, FW_NORMAL, HFONT, OUT_DEFAULT_PRECIS,
};
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PCWSTR, PWSTR, Result, w};
use yy_config::TransferConfig;
use yy_remote::log::TransferLog;
use yy_remote::sftp::Sftp;
use yy_remote::xfer::{self, Direction, Job, Journal, State};
use yy_remote::{RemoteUri, Transport};

use super::{
    ID_BOTTOM_TABS, ID_CANCEL_JOB, ID_CLEAR_DONE, ID_JOBS, ID_LOG, ID_OPEN_LOCAL, ID_PAUSE,
    ID_RESUME, ID_RESUME_ALL, ID_SHOW_JOBS, ID_SHOW_LOG, ID_TRANSFER_LOG, WM_APP_XFER_EVENT,
    WM_APP_XFER_REFRESH, with,
};
use crate::util::{Context, error_box, info_box};

/// 画面の記録の上限（超えたら古い方から消す。ファイルにはすべて残る）
const LOG_LIMIT: usize = 512 * 1024;
/// フォルダをたどる深さの上限
const MAX_DEPTH: usize = 64;

/// 作業スレッドから UI への知らせ。
enum Event {
    /// 転送の進み・状態（`run` は送り出した回の番号）
    Progress { run: u64, job: Job },
    /// 記録の行
    Line(String),
}

/// 作業スレッドへの 1 件。
struct Work {
    run: u64,
    job: Job,
    cancel: Arc<AtomicBool>,
    /// 取り消し（終わったら途中のファイルとジャーナルを消す）
    discard: Arc<AtomicBool>,
    connect: Option<crate::remote::BackgroundConnect>,
    /// 一覧に使っている接続（同じ SSH の接続にチャネルを足す）
    transport: Option<Arc<dyn Transport>>,
    retry: xfer::Retry,
    scp_chunk: u64,
}

/// UI への知らせの口（溜まっている間はフレームに 1 回だけ知らせる）。
struct Notify {
    frame: isize,
    pending: AtomicBool,
    tx: mpsc::Sender<Event>,
}

impl Notify {
    fn send(&self, e: Event) {
        let _ = self.tx.send(e);
        if !self.pending.swap(true, Ordering::AcqRel) {
            unsafe {
                let _ = PostMessageW(
                    Some(HWND(self.frame as *mut _)),
                    WM_APP_XFER_EVENT,
                    WPARAM(0),
                    LPARAM(0),
                );
            }
        }
    }
}

/// 一覧の 1 行。
struct Entry {
    job: Job,
    run: u64,
    cancel: Arc<AtomicBool>,
    discard: Arc<AtomicBool>,
    /// 速度（バイト/秒）と、計った時刻・量
    speed: f64,
    last: Option<(Instant, u64)>,
}

impl Entry {
    fn active(&self) -> bool {
        matches!(self.job.state, State::Queued | State::Running)
    }

    fn resumable(&self) -> bool {
        matches!(self.job.state, State::Interrupted | State::Failed)
    }
}

pub(super) struct Queue {
    frame: HWND,
    tabs: HWND,
    jobs: HWND,
    log: HWND,
    log_font: HFONT,
    entries: Vec<Entry>,
    tx: Option<mpsc::Sender<Work>>,
    events: mpsc::Receiver<Event>,
    notify: Arc<Notify>,
    worker: Option<std::thread::JoinHandle<()>>,
    journal: Option<Arc<Journal>>,
    tlog: Arc<TransferLog>,
    next_id: u64,
    next_run: u64,
    log_len: usize,
    /// 送り終えたファイル（表示しているフォルダなら一覧を読み直す）
    refresh: Vec<RemoteUri>,
}

/// 転送の記録のファイル（設定のフォルダの `logs\transfer.log`）。
pub(super) fn log_path() -> Option<PathBuf> {
    Some(yy_config::config_dir()?.join("logs").join("transfer.log"))
}

const COLUMNS: [(&str, i32, bool); 8] = [
    ("名前", 220, false),
    ("向き", 90, false),
    ("方式", 50, false),
    ("大きさ", 90, true),
    ("進み", 140, true),
    ("速度", 90, true),
    ("状態", 240, false),
    ("場所", 420, false),
];

impl Queue {
    pub(super) fn create(
        frame: HWND,
        instance: HINSTANCE,
        ui_font: HFONT,
        dpi: u32,
    ) -> Result<Queue> {
        unsafe {
            let child = |class: PCWSTR, style: WINDOW_STYLE, ex: WINDOW_EX_STYLE, id: u16| {
                let h = CreateWindowExW(
                    ex,
                    class,
                    None,
                    WS_CHILD | style,
                    0,
                    0,
                    0,
                    0,
                    Some(frame),
                    Some(HMENU(id as isize as *mut _)),
                    Some(instance),
                    None,
                )?;
                SendMessageW(
                    h,
                    WM_SETFONT,
                    Some(WPARAM(ui_font.0 as usize)),
                    Some(LPARAM(1)),
                );
                Ok::<_, windows::core::Error>(h)
            };
            let tabs = child(
                WC_TABCONTROLW,
                WS_VISIBLE | WS_CLIPSIBLINGS,
                WINDOW_EX_STYLE::default(),
                ID_BOTTOM_TABS,
            )
            .context("CreateWindowExW(tabs)")?;
            for (i, text) in ["転送", "記録"].iter().enumerate() {
                let t = crate::util::wide(text);
                let item = TCITEMW {
                    mask: TCIF_TEXT,
                    pszText: PWSTR(t.as_ptr() as *mut _),
                    ..Default::default()
                };
                SendMessageW(
                    tabs,
                    TCM_INSERTITEMW,
                    Some(WPARAM(i)),
                    Some(LPARAM(&item as *const _ as isize)),
                );
            }
            let jobs = child(
                WC_LISTVIEWW,
                WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(LVS_REPORT | LVS_SHOWSELALWAYS),
                WS_EX_CLIENTEDGE,
                ID_JOBS,
            )
            .context("CreateWindowExW(jobs)")?;
            let _ = windows::Win32::UI::Controls::SetWindowTheme(jobs, w!("Explorer"), None);
            SendMessageW(
                jobs,
                LVM_SETEXTENDEDLISTVIEWSTYLE,
                Some(WPARAM(0)),
                Some(LPARAM(
                    (LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER) as isize,
                )),
            );
            for (i, (text, width, right)) in COLUMNS.iter().enumerate() {
                let t = crate::util::wide(text);
                let col = LVCOLUMNW {
                    mask: LVCF_TEXT | LVCF_WIDTH | LVCF_FMT,
                    fmt: if *right { LVCFMT_RIGHT } else { LVCFMT_LEFT },
                    cx: width * dpi as i32 / 96,
                    pszText: PWSTR(t.as_ptr() as *mut _),
                    ..Default::default()
                };
                SendMessageW(
                    jobs,
                    LVM_INSERTCOLUMNW,
                    Some(WPARAM(i)),
                    Some(LPARAM(&col as *const _ as isize)),
                );
            }
            let log = child(
                w!("EDIT"),
                WS_TABSTOP
                    | WS_VSCROLL
                    | WS_HSCROLL
                    | WINDOW_STYLE(
                        (ES_MULTILINE | ES_READONLY | ES_AUTOVSCROLL | ES_AUTOHSCROLL) as u32,
                    ),
                WS_EX_CLIENTEDGE,
                ID_LOG,
            )
            .context("CreateWindowExW(log)")?;
            // 記録は等幅（同梱の UDEV Gothic）
            let log_font = CreateFontW(
                -(13 * dpi as i32 / 96),
                0,
                0,
                0,
                FW_NORMAL.0 as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY,
                (FIXED_PITCH.0 | FF_MODERN.0) as u32,
                w!("UDEV Gothic"),
            );
            if !log_font.is_invalid() {
                SendMessageW(
                    log,
                    WM_SETFONT,
                    Some(WPARAM(log_font.0 as usize)),
                    Some(LPARAM(1)),
                );
            }
            SendMessageW(log, EM_SETLIMITTEXT, Some(WPARAM(0)), None);

            let (etx, erx) = mpsc::channel();
            let notify = Arc::new(Notify {
                frame: frame.0 as isize,
                pending: AtomicBool::new(false),
                tx: etx,
            });
            let tlog = Arc::new(TransferLog::new(log_path(), crate::remote::local_clock));
            let n = notify.clone();
            tlog.set_sink(Box::new(move |l| n.send(Event::Line(l.to_owned()))));
            let journal =
                yy_config::config_dir().map(|d| Arc::new(Journal::new(d.join("transfers"))));
            let next_id = journal.as_ref().map_or(1, |j| j.next_id());
            let (wtx, wrx) = mpsc::channel::<Work>();
            let worker = {
                let notify = notify.clone();
                let tlog = tlog.clone();
                let journal = journal.clone();
                std::thread::Builder::new()
                    .name("yysftp-transfer".into())
                    .spawn(move || worker(wrx, notify, tlog, journal))
                    .ok()
            };
            tlog.line(
                None,
                &format!(
                    "yysftp {} を起動しました（記録: {}、ジャーナル: {}）",
                    env!("CARGO_PKG_VERSION"),
                    log_path()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "なし".into()),
                    journal
                        .as_ref()
                        .map(|j| j.dir().display().to_string())
                        .unwrap_or_else(|| "なし".into()),
                ),
            );
            Ok(Queue {
                frame,
                tabs,
                jobs,
                log,
                log_font,
                entries: Vec::new(),
                tx: Some(wtx),
                events: erx,
                notify,
                worker,
                journal,
                tlog,
                next_id,
                next_run: 1,
                log_len: 0,
                refresh: Vec::new(),
            })
        }
    }

    /// 下の欄の子ウィンドウの位置（タブの行と、その下の一覧・記録）。
    pub(super) fn layout(&self, r: RECT, dpi: i32) -> Vec<(HWND, RECT)> {
        let mut item = RECT::default();
        let ok = unsafe {
            SendMessageW(
                self.tabs,
                TCM_GETITEMRECT,
                Some(WPARAM(0)),
                Some(LPARAM(&mut item as *mut _ as isize)),
            )
            .0 != 0
        };
        let tab_h = if ok && item.bottom > 0 {
            item.bottom + 2
        } else {
            26 * dpi / 96
        };
        let body = RECT {
            left: r.left,
            top: r.top + tab_h,
            right: r.right,
            bottom: r.bottom,
        };
        vec![
            (
                self.tabs,
                RECT {
                    left: r.left,
                    top: r.top,
                    right: r.right,
                    bottom: r.top + tab_h,
                },
            ),
            (self.jobs, body),
            (self.log, body),
        ]
    }

    fn show_tab(&self, i: usize) {
        unsafe {
            SendMessageW(self.tabs, TCM_SETCURSEL, Some(WPARAM(i)), None);
            let _ = ShowWindow(self.jobs, if i == 0 { SW_SHOW } else { SW_HIDE });
            let _ = ShowWindow(self.log, if i == 1 { SW_SHOW } else { SW_HIDE });
        }
        if i == 1 {
            self.scroll_log();
        }
    }

    fn current_tab(&self) -> usize {
        unsafe { SendMessageW(self.tabs, TCM_GETCURSEL, None, None).0.max(0) as usize }
    }

    /// 転送中・待機中の転送があるか。
    pub(super) fn is_running(&self) -> bool {
        self.entries.iter().any(Entry::active)
    }

    /// 終了: 転送を一時停止して（ジャーナルに残す）、作業スレッドを少し待つ。
    pub(super) fn shutdown(&mut self) {
        for e in &self.entries {
            e.cancel.store(true, Ordering::Relaxed);
        }
        self.tx = None;
        if let Some(h) = self.worker.take() {
            let until = Instant::now() + Duration::from_secs(3);
            while !h.is_finished() && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(50));
            }
            if h.is_finished() {
                let _ = h.join();
            }
        }
        self.tlog.line(None, "yysftp を終了しました");
        unsafe {
            let _ = DeleteObject(self.log_font.into());
        }
    }

    // ---- 知らせの処理 -------------------------------------------------------------

    pub(super) fn on_events(&mut self) {
        self.notify.pending.store(false, Ordering::Release);
        let mut lines = String::new();
        let mut finished_uploads = Vec::new();
        while let Ok(e) = self.events.try_recv() {
            match e {
                Event::Line(l) => {
                    for l in l.lines() {
                        lines.push_str(l);
                        lines.push_str("\r\n");
                    }
                }
                Event::Progress { run, job } => {
                    if job.state == State::Done && job.direction == Direction::Upload {
                        finished_uploads.push(job.remote.clone());
                    }
                    self.update(run, job);
                }
            }
        }
        if !lines.is_empty() {
            self.append_log(&lines);
        }
        if !finished_uploads.is_empty() {
            self.refresh_if_shown(&finished_uploads);
        }
    }

    /// 送り終えたファイルのフォルダを表示していれば、一覧を読み直すよう知らせる（状態を
    /// 借りている最中なので、フレームに知らせて後で確かめる）。
    fn refresh_if_shown(&mut self, done: &[RemoteUri]) {
        self.refresh.extend_from_slice(done);
        unsafe {
            let _ = PostMessageW(Some(self.frame), WM_APP_XFER_REFRESH, WPARAM(0), LPARAM(0));
        }
    }

    /// 送り終えたファイルが `loc` のフォルダにあり、そのフォルダへの転送が残っていなければ `true`。
    pub(super) fn take_refresh(&mut self, loc: Option<&super::Loc>) -> bool {
        let done = std::mem::take(&mut self.refresh);
        let Some(loc) = loc else {
            return false;
        };
        let parent = |u: &RemoteUri| {
            let p = &u.path;
            let i = p.iter().rposition(|&b| b == b'/').unwrap_or(0);
            if i == 0 {
                b"/".to_vec()
            } else {
                p[..i].to_vec()
            }
        };
        let here = |u: &RemoteUri| u.target().same(&loc.target) && parent(u) == loc.path;
        done.iter().any(here)
            && !self
                .entries
                .iter()
                .any(|e| e.active() && e.job.direction == Direction::Upload && here(&e.job.remote))
    }

    fn update(&mut self, run: u64, job: Job) {
        let Some(i) = self.entries.iter().position(|e| e.job.id == job.id) else {
            return;
        };
        let e = &mut self.entries[i];
        if e.run != run {
            // 前の回（一時停止した後に再開したなど）の知らせ
            return;
        }
        let now = Instant::now();
        if job.state == State::Running {
            match e.last {
                Some((t, n)) if now.duration_since(t) >= Duration::from_millis(500) => {
                    let dt = now.duration_since(t).as_secs_f64();
                    let v = job.current.saturating_sub(n) as f64 / dt;
                    e.speed = if e.speed == 0.0 {
                        v
                    } else {
                        e.speed * 0.6 + v * 0.4
                    };
                    e.last = Some((now, job.current));
                }
                Some(_) => {}
                None => e.last = Some((now, job.current)),
            }
        } else {
            e.speed = 0.0;
            e.last = None;
        }
        e.job = job;
        self.update_row(i);
    }

    fn row_texts(e: &Entry) -> [String; 8] {
        let j = &e.job;
        let name = match j.direction {
            Direction::Upload => j
                .local
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            Direction::Download => {
                let p = &j.remote.path;
                yy_remote::display(p.rsplit(|&b| b == b'/').next().unwrap_or(p))
            }
        };
        let pos = if j.state == State::Done {
            j.size
        } else {
            j.current.max(j.done)
        };
        let pct = if j.size == 0 {
            if j.state == State::Done { 100.0 } else { 0.0 }
        } else {
            pos as f64 * 100.0 / j.size as f64
        };
        let state = if j.message.is_empty() {
            j.state.label().to_owned()
        } else {
            format!("{}: {}", j.state.label(), j.message)
        };
        [
            name,
            match j.direction {
                Direction::Upload => "↑ アップロード".into(),
                Direction::Download => "↓ ダウンロード".into(),
            },
            j.protocol.name().into(),
            xfer::human(j.size),
            format!("{pct:.1}%  {}", xfer::human(pos)),
            if j.state == State::Running && e.speed > 0.0 {
                format!("{}/s", xfer::human(e.speed as u64))
            } else {
                String::new()
            },
            state,
            j.label(),
        ]
    }

    fn update_row(&self, i: usize) {
        let texts = Self::row_texts(&self.entries[i]);
        for (c, t) in texts.iter().enumerate() {
            super::set_cell(self.jobs, i, c, t);
        }
    }

    fn insert_row(&self, i: usize) {
        let texts = Self::row_texts(&self.entries[i]);
        let t = crate::util::wide(&texts[0]);
        let lv = LVITEMW {
            mask: LVIF_TEXT,
            iItem: i as i32,
            pszText: PWSTR(t.as_ptr() as *mut _),
            ..Default::default()
        };
        unsafe {
            SendMessageW(
                self.jobs,
                LVM_INSERTITEMW,
                None,
                Some(LPARAM(&lv as *const _ as isize)),
            );
        }
        for (c, t) in texts.iter().enumerate().skip(1) {
            super::set_cell(self.jobs, i, c, t);
        }
    }

    fn append_log(&mut self, text: &str) {
        let w = crate::util::wide(text);
        unsafe {
            let len = SendMessageW(self.log, WM_GETTEXTLENGTH, None, None).0 as usize;
            if len + w.len() > LOG_LIMIT {
                // 古い方の 1/4 を消す
                let cut = (len / 4).max(len + w.len() - LOG_LIMIT).min(len);
                SendMessageW(
                    self.log,
                    EM_SETSEL,
                    Some(WPARAM(0)),
                    Some(LPARAM(cut as isize)),
                );
                SendMessageW(
                    self.log,
                    EM_REPLACESEL,
                    Some(WPARAM(0)),
                    Some(LPARAM(w!("").as_ptr() as isize)),
                );
            }
            let len = SendMessageW(self.log, WM_GETTEXTLENGTH, None, None).0 as usize;
            SendMessageW(
                self.log,
                EM_SETSEL,
                Some(WPARAM(len)),
                Some(LPARAM(len as isize)),
            );
            SendMessageW(
                self.log,
                EM_REPLACESEL,
                Some(WPARAM(0)),
                Some(LPARAM(w.as_ptr() as isize)),
            );
            self.log_len = len + w.len();
        }
    }

    fn scroll_log(&self) {
        unsafe {
            let len = SendMessageW(self.log, WM_GETTEXTLENGTH, None, None).0 as usize;
            SendMessageW(
                self.log,
                EM_SETSEL,
                Some(WPARAM(len)),
                Some(LPARAM(len as isize)),
            );
            SendMessageW(self.log, EM_SCROLLCARET, None, None);
        }
    }

    // ---- 追加・再開・一時停止・取り消し ---------------------------------------------

    fn take_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// 一覧に加える（まだ送らない）。
    fn add(&mut self, job: Job) -> usize {
        self.entries.push(Entry {
            job,
            run: 0,
            cancel: Arc::new(AtomicBool::new(false)),
            discard: Arc::new(AtomicBool::new(false)),
            speed: 0.0,
            last: None,
        });
        let i = self.entries.len() - 1;
        self.insert_row(i);
        i
    }

    /// `i` 番目を作業スレッドに送る。
    fn send(
        &mut self,
        i: usize,
        transport: Option<Arc<dyn Transport>>,
        cfg: &TransferConfig,
    ) -> std::result::Result<(), String> {
        let target = self.entries[i].job.remote.target();
        let connect = crate::remote::background_connector(&target)?;
        let run = self.next_run;
        self.next_run += 1;
        let e = &mut self.entries[i];
        e.run = run;
        e.cancel = Arc::new(AtomicBool::new(false));
        e.discard = Arc::new(AtomicBool::new(false));
        e.job.state = State::Queued;
        e.job.message.clear();
        e.speed = 0.0;
        e.last = None;
        if let Some(j) = &self.journal
            && let Err(err) = j.save(&e.job)
        {
            self.tlog
                .line(Some(e.job.id), &format!("ジャーナルを書けません: {err}"));
        }
        let retry = xfer::Retry {
            attempts: cfg.retries.max(1),
            ..Default::default()
        };
        let work = Work {
            run,
            job: e.job.clone(),
            cancel: e.cancel.clone(),
            discard: e.discard.clone(),
            connect: Some(connect),
            transport,
            retry,
            scp_chunk: u64::from(cfg.scp_chunk_mb.max(1)) << 20,
        };
        self.tlog.line(
            Some(work.job.id),
            &format!("待ち行列に入れました: {}", work.job.label()),
        );
        self.update_row(i);
        self.tx
            .as_ref()
            .ok_or_else(|| "終了しています".to_owned())?
            .send(work)
            .map_err(|_| "転送のスレッドが止まっています".to_owned())
    }

    /// 選んでいる行（なければ空）。
    fn selected(&self) -> Vec<usize> {
        let mut out = Vec::new();
        let mut i = -1isize;
        loop {
            i = unsafe {
                SendMessageW(
                    self.jobs,
                    LVM_GETNEXTITEM,
                    Some(WPARAM(i as usize)),
                    Some(LPARAM(LVNI_SELECTED as isize)),
                )
                .0
            };
            if i < 0 {
                break;
            }
            out.push(i as usize);
        }
        out
    }

    fn pause(&mut self, rows: &[usize]) {
        for &i in rows {
            let Some(e) = self.entries.get_mut(i) else {
                continue;
            };
            if !e.active() {
                continue;
            }
            e.cancel.store(true, Ordering::Relaxed);
            if e.job.state == State::Queued {
                // まだ始まっていない（作業スレッドは始めずに飛ばす）
                e.job.state = State::Interrupted;
                e.job.message = "一時停止しました".into();
                e.run = 0;
                if let Some(j) = &self.journal {
                    let _ = j.save(&e.job);
                }
                self.tlog.line(Some(e.job.id), "一時停止しました（開始前）");
                self.update_row(i);
            } else {
                self.tlog.line(Some(e.job.id), "一時停止を指示しました");
            }
        }
    }

    /// 一覧から外して、作業スレッドに途中のファイルとジャーナルを消させる。
    fn cancel(
        &mut self,
        mut rows: Vec<usize>,
        transport: impl Fn(&RemoteUri) -> Option<Arc<dyn Transport>>,
    ) {
        rows.sort_unstable();
        rows.dedup();
        for &i in rows.iter().rev() {
            if i >= self.entries.len() {
                continue;
            }
            let e = self.entries.remove(i);
            unsafe {
                SendMessageW(self.jobs, LVM_DELETEITEM, Some(WPARAM(i)), None);
            }
            self.tlog.line(Some(e.job.id), "取り消しを指示しました");
            e.discard.store(true, Ordering::Relaxed);
            e.cancel.store(true, Ordering::Relaxed);
            if e.active() && e.run != 0 {
                // 作業スレッドが止めてから片付ける
                continue;
            }
            let work = Work {
                run: 0,
                transport: transport(&e.job.remote),
                job: e.job,
                cancel: e.cancel,
                discard: e.discard,
                connect: None,
                retry: xfer::Retry::default(),
                scp_chunk: 0,
            };
            if let Some(tx) = &self.tx {
                let _ = tx.send(work);
            }
        }
    }

    fn clear_done(&mut self) {
        for i in (0..self.entries.len()).rev() {
            if self.entries[i].job.state == State::Done {
                self.entries.remove(i);
                unsafe {
                    SendMessageW(self.jobs, LVM_DELETEITEM, Some(WPARAM(i)), None);
                }
            }
        }
    }

    /// 同じファイルの転送がすでに一覧にあれば、その行。
    fn find_same(&self, direction: Direction, local: &Path, remote: &RemoteUri) -> Option<usize> {
        self.entries.iter().position(|e| {
            e.job.direction == direction
                && e.job.state != State::Done
                && e.job.local == local
                && e.job.remote.same(remote)
                && e.job.remote.path == remote.path
        })
    }
}

// ---- 作業スレッド -----------------------------------------------------------------

fn worker(
    rx: mpsc::Receiver<Work>,
    notify: Arc<Notify>,
    tlog: Arc<TransferLog>,
    journal: Option<Arc<Journal>>,
) {
    // 前の転送の接続（同じ接続先なら使い回す）
    let mut reuse: Option<(yy_remote::uri::Target, Arc<dyn Transport>)> = None;
    while let Ok(w) = rx.recv() {
        let mut job = w.job;
        let target = job.remote.target();
        let mut used = w.transport.clone().filter(|t| !t.is_closed()).or_else(|| {
            reuse
                .as_ref()
                .filter(|(r, t)| r.same(&target) && !t.is_closed())
                .map(|(_, t)| t.clone())
        });
        if w.cancel.load(Ordering::Relaxed) || w.connect.is_none() {
            if !w.discard.load(Ordering::Relaxed) {
                job.state = State::Interrupted;
                job.message = "一時停止しました".into();
                if let Some(j) = &journal {
                    let _ = j.save(&job);
                }
            }
        } else if let Some(connect) = &w.connect {
            let n = notify.clone();
            let run = w.run;
            let mut progress = move |j: &Job| {
                n.send(Event::Progress {
                    run,
                    job: j.clone(),
                })
            };
            let mut cx = xfer::Context {
                connect: &**connect,
                log: &tlog,
                journal: journal.as_deref(),
                cancel: &w.cancel,
                progress: &mut progress,
                retry: w.retry.clone(),
                scp_chunk: w.scp_chunk,
                transport: used.clone(),
            };
            xfer::run(&mut job, &mut cx);
            if let Some(t) = cx.transport.take() {
                used = Some(t.clone());
                reuse = Some((target.clone(), t));
            }
        }
        if w.discard.load(Ordering::Relaxed) && job.state != State::Done {
            discard(&job, journal.as_deref(), &tlog, used.as_deref());
            continue;
        }
        notify.send(Event::Progress { run: w.run, job });
    }
}

/// 取り消した転送の途中のファイルとジャーナルを消す。
fn discard(job: &Job, journal: Option<&Journal>, tlog: &TransferLog, t: Option<&dyn Transport>) {
    if let Some(j) = journal {
        let _ = j.remove(job.id);
    }
    match job.direction {
        Direction::Download => {
            let part = job.local_part();
            match std::fs::remove_file(&part) {
                Ok(()) => tlog.line(
                    Some(job.id),
                    &format!(
                        "取り消しました。途中のファイル {} を消しました",
                        part.display()
                    ),
                ),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    tlog.line(Some(job.id), "取り消しました");
                }
                Err(e) => tlog.line(
                    Some(job.id),
                    &format!(
                        "取り消しました。途中のファイル {} を消せません: {e}",
                        part.display()
                    ),
                ),
            }
        }
        Direction::Upload => {
            let part = job.remote_part();
            let shown = yy_remote::display(&part);
            let r = match t.filter(|t| !t.is_closed()) {
                Some(t) => {
                    let mut cmd = b"rm -f -- ".to_vec();
                    cmd.extend_from_slice(&yy_remote::shell_quote(&part));
                    yy_remote::scp::shell(t, &cmd, "途中のファイルの削除")
                }
                None => Err(std::io::Error::other("接続していません")),
            };
            match r {
                Ok(()) => tlog.line(
                    Some(job.id),
                    &format!("取り消しました。接続先の途中のファイル {shown} を消しました"),
                ),
                Err(e) => tlog.line(
                    Some(job.id),
                    &format!(
                        "取り消しました。接続先に途中のファイル {shown} が残っています（{e}）"
                    ),
                ),
            }
        }
    }
}

// ---- UI からの操作 -----------------------------------------------------------------

pub(super) fn on_notify(hwnd: HWND, hdr: &NMHDR, _lparam: LPARAM) -> LRESULT {
    match (hdr.idFrom as u16, hdr.code) {
        (ID_BOTTOM_TABS, TCN_SELCHANGE) => {
            with(|a| {
                let i = a.queue.current_tab();
                a.queue.show_tab(i);
            });
        }
        (ID_JOBS, NM_RCLICK) => jobs_menu(hwnd),
        (ID_JOBS, NM_DBLCLK) => command(hwnd, ID_OPEN_LOCAL),
        _ => {}
    }
    LRESULT(0)
}

fn jobs_menu(hwnd: HWND) {
    let Some((sel, any_resumable, any_done)) = with(|a| {
        let q = &a.queue;
        let sel = q.selected();
        (
            sel,
            q.entries.iter().any(Entry::resumable),
            q.entries.iter().any(|e| e.job.state == State::Done),
        )
    }) else {
        return;
    };
    let mut pt = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut pt);
        let Ok(menu) = CreatePopupMenu() else { return };
        let item = |id: u16, text: &str, on: bool| {
            let t = crate::util::wide(text);
            let flags = if on { MF_STRING } else { MF_STRING | MF_GRAYED };
            AppendMenuW(menu, flags, id as usize, PCWSTR(t.as_ptr()))
        };
        let sep = || AppendMenuW(menu, MF_SEPARATOR, 0, None);
        let has = !sel.is_empty();
        let _ = item(ID_PAUSE, "一時停止(&P)", true);
        let _ = item(ID_RESUME, "再開(&R)", has);
        let _ = item(ID_CANCEL_JOB, "取り消し(&C)", has);
        let _ = sep();
        let _ = item(ID_OPEN_LOCAL, "手元のフォルダを開く(&O)", sel.len() == 1);
        let _ = sep();
        let _ = item(ID_RESUME_ALL, "すべて再開(&A)", any_resumable);
        let _ = item(ID_CLEAR_DONE, "完了したものを一覧から消す(&L)", any_done);
        let _ = item(ID_TRANSFER_LOG, "転送の記録を開く(&T)", true);
        let cmd = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON,
            pt.x,
            pt.y,
            None,
            hwnd,
            None,
        );
        let _ = DestroyMenu(menu);
        if cmd.0 != 0 {
            super::command(hwnd, cmd.0 as u16);
        }
    }
}

/// 一覧に使っている接続（あれば）。
fn browsing_transport(uri: &RemoteUri) -> Option<Arc<dyn Transport>> {
    let target = uri.target();
    with(|a| a.browser(&target).map(|b| b.transport.clone())).flatten()
}

/// 転送のメニュー・一覧の右クリックの操作。
pub(super) fn command(hwnd: HWND, id: u16) {
    match id {
        ID_SHOW_JOBS | ID_SHOW_LOG => {
            with(|a| a.queue.show_tab(usize::from(id == ID_SHOW_LOG)));
        }
        ID_PAUSE => {
            with(|a| {
                let mut rows = a.queue.selected();
                if rows.is_empty() {
                    rows = (0..a.queue.entries.len()).collect();
                }
                a.queue.pause(&rows);
            });
        }
        ID_RESUME | ID_RESUME_ALL => {
            let rows = with(|a| {
                let q = &a.queue;
                let rows: Vec<usize> = if id == ID_RESUME_ALL {
                    (0..q.entries.len()).collect()
                } else {
                    q.selected()
                };
                rows.into_iter()
                    .filter(|&i| q.entries.get(i).is_some_and(Entry::resumable))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
            let mut errors = Vec::new();
            for i in rows {
                let uri = with(|a| a.queue.entries.get(i).map(|e| e.job.remote.clone())).flatten();
                let Some(uri) = uri else { continue };
                let t = browsing_transport(&uri);
                if let Some(Err(e)) = with(|a| {
                    let cfg = a.config.transfer.clone();
                    a.queue.send(i, t, &cfg)
                }) {
                    errors.push(e);
                }
            }
            if !errors.is_empty() {
                errors.dedup();
                error_box(
                    hwnd,
                    &format!("再開できませんでした。\n{}", errors.join("\n")),
                );
            }
        }
        ID_CANCEL_JOB => {
            let Some(rows) = with(|a| a.queue.selected()) else {
                return;
            };
            if rows.is_empty() {
                return;
            }
            let r = unsafe {
                MessageBoxW(
                    Some(hwnd),
                    &HSTRING::from(format!(
                        "選んだ {} 件の転送を取り消しますか？\n途中まで送ったファイル（.yypart）も消します。",
                        rows.len()
                    )),
                    w!("yysftp"),
                    MB_OKCANCEL | MB_ICONQUESTION,
                )
            };
            if r != IDOK {
                return;
            }
            // 接続は先に集める（取り消しの中で状態を借り直さない）
            let transports: Vec<(RemoteUri, Option<Arc<dyn Transport>>)> = with(|a| {
                rows.iter()
                    .filter_map(|&i| a.queue.entries.get(i))
                    .map(|e| {
                        let t = e.job.remote.target();
                        (
                            e.job.remote.clone(),
                            a.browser(&t).map(|b| b.transport.clone()),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
            with(|a| {
                a.queue.cancel(rows, |u| {
                    transports
                        .iter()
                        .find(|(x, _)| x.same(u))
                        .and_then(|(_, t)| t.clone())
                })
            });
        }
        ID_CLEAR_DONE => {
            with(|a| a.queue.clear_done());
        }
        ID_OPEN_LOCAL => {
            let path = with(|a| {
                let q = &a.queue;
                let i = *q.selected().first()?;
                let j = &q.entries.get(i)?.job;
                Some(if j.local.exists() {
                    j.local.clone()
                } else if j.local_part().exists() {
                    j.local_part()
                } else {
                    j.local.parent()?.to_owned()
                })
            })
            .flatten();
            if let Some(p) = path {
                let _ = std::process::Command::new("explorer.exe")
                    .arg(format!("/select,{}", p.display()))
                    .spawn();
            }
        }
        _ => {}
    }
}

/// 起動したとき: ジャーナルに残っている転送を一覧に並べ、再開するか尋ねる。
pub(super) fn offer_resume(frame: HWND) {
    let Some(n) = with(|a| {
        let jobs = a
            .queue
            .journal
            .as_ref()
            .map(|j| j.load())
            .unwrap_or_default();
        let n = jobs.len();
        for mut job in jobs {
            if job.state == State::Queued || job.state == State::Running {
                job.state = State::Interrupted;
            }
            job.current = job.done;
            a.queue.tlog.line(
                Some(job.id),
                &format!(
                    "ジャーナルから読みました: {}（{} / {}、{}）",
                    job.label(),
                    xfer::human(job.done),
                    xfer::human(job.size),
                    job.state.label()
                ),
            );
            a.queue.add(job);
        }
        n
    }) else {
        return;
    };
    if n == 0 {
        return;
    }
    let text = format!(
        "前回終わらなかった転送が {n} 件あります。続きから再開しますか？\n\
         （「いいえ」を選んでも、転送の一覧の右クリックの「再開」で後から続けられます）"
    );
    let r = unsafe {
        MessageBoxW(
            Some(frame),
            &HSTRING::from(text),
            w!("yysftp"),
            MB_YESNO | MB_ICONQUESTION,
        )
    };
    if r == IDYES {
        command(frame, ID_RESUME_ALL);
    }
}

/// 同じ名前のファイルがあるときの答え。
enum Conflict {
    Overwrite,
    Skip,
}

fn ask_conflict(owner: HWND, names: &[String]) -> Option<Conflict> {
    if names.is_empty() {
        return Some(Conflict::Skip);
    }
    let list: Vec<&str> = names.iter().take(10).map(String::as_str).collect();
    let more = if names.len() > 10 {
        format!("\n…ほか {} 件", names.len() - 10)
    } else {
        String::new()
    };
    let text = format!(
        "送り先に同じ名前のファイルが {} 個あります。\n\n{}{more}\n\n\
         「はい」: 置き換える　「いいえ」: そのファイルは飛ばす　「キャンセル」: 転送をやめる",
        names.len(),
        list.join("\n")
    );
    let r = unsafe {
        MessageBoxW(
            Some(owner),
            &HSTRING::from(text),
            w!("yysftp"),
            MB_YESNOCANCEL | MB_ICONQUESTION,
        )
    };
    match r {
        IDYES => Some(Conflict::Overwrite),
        IDNO => Some(Conflict::Skip),
        _ => None,
    }
}

/// 手元のフォルダをたどる: 作るフォルダ（`base` からの相対）と、送るファイル。
fn walk_local(
    dir: &Path,
    rel: &[u8],
    depth: usize,
    dirs: &mut Vec<Vec<u8>>,
    files: &mut Vec<(PathBuf, Vec<u8>)>,
) {
    dirs.push(rel.to_vec());
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name().to_string_lossy().into_owned();
        let child = yy_remote::join_remote(rel, name.as_bytes());
        let path = e.path();
        match std::fs::metadata(&path) {
            Ok(m) if m.is_dir() => walk_local(&path, &child, depth + 1, dirs, files),
            Ok(m) if m.is_file() => files.push((path, child)),
            _ => {}
        }
    }
}

/// 手元のファイル・フォルダを、表示している接続先のフォルダに送る。
pub(super) fn upload(hwnd: HWND, paths: Vec<PathBuf>) {
    if paths.is_empty() {
        return;
    }
    let Some(Some((loc, protocol))) = with(|a| a.loc.clone().map(|l| (l, a.protocol))) else {
        info_box(hwnd, "先に送り先（接続先のフォルダ）を開いてください。");
        return;
    };
    // 送るもの（接続先のパスは表示しているフォルダからの相対）
    let mut dirs: Vec<Vec<u8>> = Vec::new();
    let mut files: Vec<(PathBuf, Vec<u8>)> = Vec::new();
    for p in &paths {
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        match std::fs::metadata(p) {
            Ok(m) if m.is_dir() => walk_local(p, name.as_bytes(), 0, &mut dirs, &mut files),
            Ok(_) => files.push((p.clone(), name.into_bytes())),
            Err(e) => {
                error_box(hwnd, &format!("{} を読めません。\n{e}", p.display()));
                return;
            }
        }
    }
    if files.is_empty() && dirs.is_empty() {
        return;
    }
    // 接続先でフォルダを作り、同じ名前のファイルがあるか確かめる
    let base = loc.path.clone();
    let mk: Vec<Vec<u8>> = dirs
        .iter()
        .map(|d| yy_remote::join_remote(&base, d))
        .collect();
    let check: Vec<Vec<u8>> = files
        .iter()
        .map(|(_, r)| yy_remote::join_remote(&base, r))
        .collect();
    let label = if dirs.is_empty() {
        "送り先を確かめています…（Esc で中止）"
    } else {
        "送り先のフォルダを作っています…（Esc で中止）"
    };
    let exists = match super::remote_op(&loc.target, label, move |s: &Sftp| {
        for d in &mk {
            match s.try_stat(d)? {
                Some(a) if a.is_dir() => {}
                Some(_) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::AlreadyExists,
                        format!("{} はフォルダではありません", yy_remote::display(d)),
                    ));
                }
                None => s.mkdir(d)?,
            }
        }
        check
            .iter()
            .map(|p| Ok(s.try_stat(p)?.is_some()))
            .collect::<std::io::Result<Vec<bool>>>()
    }) {
        Ok(v) => v,
        Err(e) => {
            error_box(hwnd, &format!("送り先を準備できませんでした。\n{e}"));
            return;
        }
    };
    let conflicts: Vec<String> = files
        .iter()
        .zip(&exists)
        .filter(|(_, e)| **e)
        .map(|((_, r), _)| yy_remote::display(r))
        .collect();
    let Some(answer) = ask_conflict(hwnd, &conflicts) else {
        return;
    };
    let mut jobs = Vec::new();
    let mut errors = Vec::new();
    for ((local, rel), exists) in files.iter().zip(&exists) {
        if *exists && matches!(answer, Conflict::Skip) {
            continue;
        }
        let remote = loc.child(rel).uri();
        match xfer::upload_job(0, protocol, local, remote, *exists) {
            Ok(j) => jobs.push(j),
            Err(e) => errors.push(format!("{}: {e}", local.display())),
        }
    }
    enqueue(hwnd, jobs, errors);
}

/// 接続先のフォルダをたどる: 作るフォルダと、受け取るファイル（パス・相対パス・大きさ・更新日時）。
type RemoteFile = (Vec<u8>, Vec<u8>, u64, u64);

fn walk_remote(
    s: &Sftp,
    path: &[u8],
    rel: &[u8],
    depth: usize,
    dirs: &mut Vec<Vec<u8>>,
    files: &mut Vec<RemoteFile>,
) -> std::io::Result<()> {
    dirs.push(rel.to_vec());
    if depth > MAX_DEPTH {
        return Ok(());
    }
    let mut entries = s.read_dir(path)?;
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    for e in entries {
        if e.name == b"." || e.name == b".." {
            continue;
        }
        let full = yy_remote::join_remote(path, &e.name);
        let child = yy_remote::join_remote(rel, &e.name);
        let attrs = if e.attrs.is_symlink() {
            // リンク先がフォルダなら（輪にならないよう）たどらない
            match s.try_stat(&full)? {
                Some(a) if a.is_dir() => continue,
                Some(a) => a,
                None => continue,
            }
        } else {
            e.attrs
        };
        if attrs.is_dir() {
            walk_remote(s, &full, &child, depth + 1, dirs, files)?;
        } else {
            files.push((
                full,
                child,
                attrs.size.unwrap_or(0),
                attrs.mtime.map_or(0, u64::from),
            ));
        }
    }
    Ok(())
}

/// ダウンロードの既定の保存先（設定、なければ「ダウンロード」フォルダ）。
fn default_download_dir() -> PathBuf {
    let cfg = with(|a| a.config.transfer.download_dir.clone()).unwrap_or_default();
    if !cfg.trim().is_empty() {
        return PathBuf::from(cfg.trim());
    }
    std::env::var_os("USERPROFILE")
        .map(|p| PathBuf::from(p).join("Downloads"))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// 一覧で選んでいる項目を `dest`（なければ既定の保存先）に受け取る。
pub(super) fn download_selected(dest: Option<PathBuf>) {
    let Some(Some((hwnd, loc, items, protocol))) = with(|a| {
        let loc = a.loc.clone()?;
        let items: Vec<super::Item> = a
            .selected()
            .iter()
            .filter_map(|&i| a.items.get(i).cloned())
            .collect();
        Some((a.frame, loc, items, a.protocol))
    }) else {
        return;
    };
    if items.is_empty() {
        return;
    }
    let dest = dest.unwrap_or_else(default_download_dir);
    let base = loc.path.clone();
    let picked: Vec<(Vec<u8>, super::Item)> =
        items.into_iter().map(|i| (i.name.clone(), i)).collect();
    let listed = super::remote_op(
        &loc.target,
        "受け取るものを調べています…（Esc で中止）",
        move |s: &Sftp| {
            let mut dirs = Vec::new();
            let mut files = Vec::new();
            for (name, item) in &picked {
                let full = yy_remote::join_remote(&base, name);
                let attrs = if item.attrs.is_symlink() {
                    s.stat(&full)?
                } else {
                    item.attrs.clone()
                };
                if attrs.is_dir() {
                    walk_remote(s, &full, name, 0, &mut dirs, &mut files)?;
                } else {
                    files.push((
                        full,
                        name.clone(),
                        attrs.size.unwrap_or(0),
                        attrs.mtime.map_or(0, u64::from),
                    ));
                }
            }
            Ok((dirs, files))
        },
    );
    let (dirs, files) = match listed {
        Ok(v) => v,
        Err(e) => {
            error_box(hwnd, &format!("受け取るものを調べられませんでした。\n{e}"));
            return;
        }
    };
    let local_of = |rel: &[u8]| -> PathBuf {
        let mut p = dest.clone();
        for part in String::from_utf8_lossy(rel)
            .split('/')
            .filter(|s| !s.is_empty())
        {
            p.push(sanitize(part));
        }
        p
    };
    if let Err(e) = std::fs::create_dir_all(&dest) {
        error_box(hwnd, &format!("{} を作れません。\n{e}", dest.display()));
        return;
    }
    for d in &dirs {
        let p = local_of(d);
        if let Err(e) = std::fs::create_dir_all(&p) {
            error_box(hwnd, &format!("{} を作れません。\n{e}", p.display()));
            return;
        }
    }
    let conflicts: Vec<String> = files
        .iter()
        .filter(|f| local_of(&f.1).exists())
        .map(|f| local_of(&f.1).display().to_string())
        .collect();
    let Some(answer) = ask_conflict(hwnd, &conflicts) else {
        return;
    };
    let mut jobs = Vec::new();
    for (full, rel, size, mtime) in &files {
        let local = local_of(rel);
        let exists = local.exists();
        if exists && matches!(answer, Conflict::Skip) {
            continue;
        }
        let mut uri = loc.uri();
        uri.path = full.clone();
        jobs.push(xfer::download_job(
            0, protocol, uri, *size, *mtime, &local, exists,
        ));
    }
    enqueue(hwnd, jobs, Vec::new());
}

/// Windows のファイル名に使えない文字を置き換える。
fn sanitize(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '\\' | '|' | '?' | '*' => '_',
            c if (c as u32) < 0x20 => '_',
            c => c,
        })
        .collect();
    let trimmed = s.trim_end_matches(['.', ' ']);
    if trimmed.is_empty() {
        "_".into()
    } else {
        trimmed.to_owned()
    }
}

/// 番号を振って一覧に加え、作業スレッドに送る。
fn enqueue(hwnd: HWND, jobs: Vec<Job>, mut errors: Vec<String>) {
    let mut count = 0usize;
    let mut total = 0u64;
    for mut job in jobs {
        let t = browsing_transport(&job.remote);
        let r = with(|a| {
            let cfg = a.config.transfer.clone();
            let q = &mut a.queue;
            let i = match q.find_same(job.direction, &job.local, &job.remote) {
                Some(i) if q.entries[i].active() => {
                    return Err(format!("{}: すでに転送中です", job.label()));
                }
                // 中断していた同じ転送は、その続きとして再開する
                Some(i) => i,
                None => {
                    job.id = q.take_id();
                    q.add(job.clone())
                }
            };
            q.send(i, t, &cfg)
        });
        match r {
            Some(Ok(())) => {
                count += 1;
                total += job.size;
            }
            Some(Err(e)) => errors.push(e),
            None => {}
        }
    }
    if count > 0 {
        with(|a| {
            a.queue.show_tab(0);
            a.set_status(&format!(
                "{count} 個のファイル（{}）を転送の一覧に入れました",
                xfer::human(total)
            ));
        });
    }
    if !errors.is_empty() {
        let more = if errors.len() > 10 {
            format!("\n…ほか {} 件", errors.len() - 10)
        } else {
            String::new()
        };
        errors.truncate(10);
        error_box(
            hwnd,
            &format!(
                "転送に加えられなかったものがあります。\n{}{more}",
                errors.join("\n")
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_windows_names() {
        assert_eq!(sanitize("a:b*c?.txt"), "a_b_c_.txt");
        assert_eq!(sanitize("name. "), "name");
        assert_eq!(sanitize("..."), "_");
        assert_eq!(sanitize("日本語.csv"), "日本語.csv");
    }

    #[test]
    fn walks_local_folders() {
        let dir = std::env::temp_dir().join(format!("yysftp-walk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("top").join("sub")).unwrap();
        std::fs::write(dir.join("top").join("a.txt"), b"a").unwrap();
        std::fs::write(dir.join("top").join("sub").join("b.txt"), b"b").unwrap();
        let (mut dirs, mut files) = (Vec::new(), Vec::new());
        walk_local(&dir.join("top"), b"top", 0, &mut dirs, &mut files);
        assert_eq!(dirs, vec![b"top".to_vec(), b"top/sub".to_vec()]);
        let rel: Vec<&[u8]> = files.iter().map(|(_, r)| r.as_slice()).collect();
        assert_eq!(rel, vec![&b"top/a.txt"[..], &b"top/sub/b.txt"[..]]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
