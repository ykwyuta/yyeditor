//! ダウンロード（19 章 4.3）: 保存先を決め（既定のフォルダか、毎回尋ねる）、進み具合を状態表示に出し、
//! 一時停止・再開・中止できる。終わったものはプロファイルのダウンロード履歴に残す。
//!
//! WebView2 の既定のダウンロードの吹き出しは出さない（`DownloadStarting` で `Handled`）。

use std::path::{Path, PathBuf};

use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::{BytesReceivedChangedEventHandler, StateChangedEventHandler};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Controls::{
    BST_CHECKED, BST_UNCHECKED, CheckDlgButton, IsDlgButtonChecked,
};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, w};
use yy_browser::history::{self, DownloadRecord, DownloadState};

use super::{App, with};
use crate::goto::{CLASS_BUTTON, CLASS_EDIT, CLASS_STATIC, Template};
use crate::preview::take_string;

/// 保存先を尋ねる（`lparam` は `Box<Ask>`）。
pub(super) const WM_APP_DOWNLOAD_ASK: u32 = WM_APP + 125;

/// 進行中のダウンロード。
pub(super) struct Download {
    pub id: u64,
    pub op: ICoreWebView2DownloadOperation,
    pub url: String,
    pub path: String,
    pub received: i64,
    /// 全体の大きさ（わからなければ 0 以下）
    pub total: i64,
    pub paused: bool,
}

impl Download {
    fn name(&self) -> String {
        Path::new(&self.path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.clone())
    }

    fn progress(&self) -> String {
        let got = crate::util::human_size(self.received.max(0) as u64);
        if self.total > 0 {
            format!(
                "{}%（{got} / {}）",
                self.received.max(0) * 100 / self.total,
                crate::util::human_size(self.total as u64)
            )
        } else {
            got
        }
    }
}

/// 保存先を尋ねるまで待たせているダウンロード。
pub(super) struct Ask {
    args: ICoreWebView2DownloadStartingEventArgs,
    deferral: ICoreWebView2Deferral,
}

/// ダウンロードの履歴のファイル（プロファイルのデータのフォルダ）。
pub(super) fn log_path(a: &App) -> PathBuf {
    super::data_folder(&a.profile).join("yybrowser-downloads.tsv")
}

/// 同じ名前のファイルがあれば「名前 (1).拡張子」のように空いている名前にする。
pub(super) fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let p = dir.join(name);
    if !p.exists() {
        return p;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_owned(), format!(".{e}")),
        _ => (name.to_owned(), String::new()),
    };
    (1..10_000)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|p| !p.exists())
        .unwrap_or(p)
}

/// `DownloadStarting`: 保存先を決めて始める（尋ねるなら後で）。
pub(super) fn on_starting(frame: HWND, args: &ICoreWebView2DownloadStartingEventArgs) {
    let Some((dir, ask)) = with(|a| (a.profiles.download_dir.clone(), a.profiles.download_ask))
    else {
        return;
    };
    unsafe {
        let _ = args.SetHandled(true);
    }
    let suggested = take_string(|p| unsafe { args.ResultFilePath(p) });
    let suggested = PathBuf::from(suggested);
    let name = suggested
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "download".into());
    let folder = if dir.trim().is_empty() {
        suggested
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default()
    } else {
        PathBuf::from(dir.trim())
    };
    if ask {
        // ダイアログは後で（イベントの中で出さない）
        if let Ok(deferral) = unsafe { args.GetDeferral() } {
            let b = Box::new(Ask {
                args: args.clone(),
                deferral,
            });
            unsafe {
                let _ = PostMessageW(
                    Some(frame),
                    WM_APP_DOWNLOAD_ASK,
                    WPARAM(0),
                    LPARAM(Box::into_raw(b) as isize),
                );
            }
            return;
        }
    }
    let _ = std::fs::create_dir_all(&folder);
    let path = unique_path(&folder, &name);
    unsafe {
        let _ = args.SetResultFilePath(&HSTRING::from(path.as_os_str()));
    }
    start(args, &path);
}

/// 保存先を尋ねる（[`WM_APP_DOWNLOAD_ASK`]）。
pub(super) fn ask(frame: HWND, b: Ask) {
    let suggested = PathBuf::from(take_string(|p| unsafe { b.args.ResultFilePath(p) }));
    let name = suggested
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "download".into());
    let dir = with(|a| a.profiles.download_dir.clone()).unwrap_or_default();
    let folder = if dir.trim().is_empty() {
        suggested
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default()
    } else {
        PathBuf::from(dir.trim())
    };
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_owned())
        .unwrap_or_default();
    let filter = if ext.is_empty() {
        ("すべてのファイル (*.*)".to_owned(), "*.*".to_owned())
    } else {
        (
            format!("{} ファイル (*.{ext})", ext.to_uppercase()),
            format!("*.{ext}"),
        )
    };
    let chosen = crate::fm::pick_file(frame, (&filter.0, &filter.1), Some(&folder), Some(&name));
    unsafe {
        match &chosen {
            Some(p) => {
                let _ = b.args.SetResultFilePath(&HSTRING::from(p.as_os_str()));
                start(&b.args, p);
            }
            None => {
                let _ = b.args.SetCancel(true);
            }
        }
        let _ = b.deferral.Complete();
    }
}

/// ダウンロードを一覧に足し、進み具合・終わりを受ける。
fn start(args: &ICoreWebView2DownloadStartingEventArgs, path: &Path) {
    let Ok(op) = (unsafe { args.DownloadOperation() }) else {
        return;
    };
    let url = take_string(|p| unsafe { op.Uri(p) });
    let mut total = 0i64;
    unsafe {
        let _ = op.TotalBytesToReceive(&mut total);
    }
    let Some(id) = with(|a| {
        a.next_download += 1;
        let id = a.next_download;
        a.downloads.push(Download {
            id,
            op: op.clone(),
            url: url.clone(),
            path: path.to_string_lossy().into_owned(),
            received: 0,
            total,
            paused: false,
        });
        id
    }) else {
        return;
    };
    let mut token = 0i64;
    unsafe {
        let _ = op.add_BytesReceivedChanged(
            &BytesReceivedChangedEventHandler::create(Box::new(move |sender, _| {
                if let Some(op) = sender {
                    let (mut got, mut total) = (0i64, 0i64);
                    let _ = op.BytesReceived(&mut got);
                    let _ = op.TotalBytesToReceive(&mut total);
                    with(|a| {
                        if let Some(d) = a.downloads.iter_mut().find(|d| d.id == id) {
                            d.received = got;
                            d.total = total;
                        }
                        a.set_status(&status_line(a));
                    });
                }
                Ok(())
            })),
            &mut token,
        );
        let _ = op.add_StateChanged(
            &StateChangedEventHandler::create(Box::new(move |sender, _| {
                if let Some(op) = sender {
                    let mut st = COREWEBVIEW2_DOWNLOAD_STATE::default();
                    let _ = op.State(&mut st);
                    let path = take_string(|p| op.ResultFilePath(p));
                    on_state(id, st, path);
                }
                Ok(())
            })),
            &mut token,
        );
    }
    with(|a| a.set_status(&status_line(a)));
}

/// 状態が変わった: 終わったら履歴に残して一覧から外す。
fn on_state(id: u64, st: COREWEBVIEW2_DOWNLOAD_STATE, path: String) {
    with(|a| {
        let Some(i) = a.downloads.iter().position(|d| d.id == id) else {
            return;
        };
        if !path.is_empty() {
            a.downloads[i].path = path;
        }
        let state = match st {
            COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED => DownloadState::Completed,
            COREWEBVIEW2_DOWNLOAD_STATE_INTERRUPTED if !a.downloads[i].paused => {
                // 中止（利用者）か失敗。中止は Cancel を呼んだときに印を付けている
                if a.cancelled.contains(&id) {
                    DownloadState::Cancelled
                } else {
                    DownloadState::Failed
                }
            }
            _ => {
                a.set_status(&status_line(a));
                return;
            }
        };
        let d = a.downloads.remove(i);
        a.cancelled.retain(|c| *c != id);
        let rec = DownloadRecord {
            time: yy_adblock::lists::now(),
            state,
            bytes: d.received.max(0) as u64,
            url: d.url.clone(),
            path: d.path.clone(),
        };
        if state == DownloadState::Completed {
            // 同じフォルダに同じ内容のファイルがあるかは別のスレッドで調べる（大きなファイルもあるので）
            a.set_status(&format!(
                "ダウンロードしました。重複を調べています: {}",
                d.name()
            ));
            let frame = a.frame.0 as isize;
            std::thread::spawn(move || {
                let dup = history::discard_if_duplicate(Path::new(&rec.path));
                let b = Box::new(Finished { rec, dup });
                let p = Box::into_raw(b);
                let ok = unsafe {
                    PostMessageW(
                        Some(HWND(frame as *mut _)),
                        WM_APP_DOWNLOAD_DONE,
                        WPARAM(0),
                        LPARAM(p as isize),
                    )
                };
                if ok.is_err() {
                    drop(unsafe { Box::from_raw(p) });
                }
            });
            return;
        }
        let _ = history::append(&log_path(a), &rec, history::MAX_DOWNLOADS);
        let msg = match state {
            DownloadState::Cancelled => format!("ダウンロードを中止しました: {}", d.name()),
            _ => format!("ダウンロードできませんでした: {}", d.name()),
        };
        a.set_status(&msg);
    });
}

/// 重複を調べ終わったダウンロード（`lparam` は `Box<Finished>`）。
pub(super) const WM_APP_DOWNLOAD_DONE: u32 = WM_APP + 126;

/// 重複を調べ終わったダウンロード。
pub(super) struct Finished {
    rec: DownloadRecord,
    /// 同じ内容の、前からあるファイル（あれば新しいほうは消した）
    dup: std::io::Result<Option<PathBuf>>,
}

/// 重複を調べ終わった: 履歴に残して知らせる。
pub(super) fn finished(f: Finished) {
    let Finished { mut rec, dup } = f;
    let name = Path::new(&rec.path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let msg = match dup {
        Ok(Some(existing)) => {
            rec.state = DownloadState::Duplicate;
            rec.path = existing.to_string_lossy().into_owned();
            let ename = existing
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            format!(
                "同じ内容のファイル「{ename}」が既にあるので、ダウンロードした「{name}」は破棄しました"
            )
        }
        Ok(None) => format!("ダウンロードしました: {name}（Ctrl+J でダウンロードの一覧）"),
        Err(e) => format!("ダウンロードしました: {name}（重複を調べられませんでした: {e}）"),
    };
    with(|a| {
        let _ = history::append(&log_path(a), &rec, history::MAX_DOWNLOADS);
        a.set_status(&msg);
    });
}

/// 進行中のダウンロードの状態表示。
fn status_line(a: &App) -> String {
    match a.downloads.as_slice() {
        [] => String::new(),
        [d] => format!(
            "ダウンロード{}: {} {}",
            if d.paused {
                "（一時停止）"
            } else {
                "中"
            },
            d.name(),
            d.progress()
        ),
        ds => {
            let got: i64 = ds.iter().map(|d| d.received.max(0)).sum();
            format!(
                "ダウンロード中: {} 個（{}）。Ctrl+J で一覧",
                ds.len(),
                crate::util::human_size(got as u64)
            )
        }
    }
}

/// ファイルを開く。
fn open_path(p: &str) {
    unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            &HSTRING::from(p),
            None,
            None,
            SW_SHOWNORMAL,
        );
    }
}

/// エクスプローラーでファイルを選んだ状態で開く。
fn reveal(p: &str) {
    let arg = format!("/select,\"{p}\"");
    unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            w!("explorer.exe"),
            &HSTRING::from(arg),
            None,
            SW_SHOWNORMAL,
        );
    }
}

// ---- ダウンロードの一覧の画面 ---------------------------------------------------------------

const D_LIST: u16 = 100;
const D_OPEN: u16 = 101;
const D_FOLDER: u16 = 102;
const D_PAUSE: u16 = 103;
const D_CANCEL: u16 = 104;
const D_REMOVE: u16 = 105;
const D_CLEAR: u16 = 106;
const D_DIR: u16 = 107;
const D_DIR_CHANGE: u16 = 108;
const D_ASK: u16 = 109;
const IDCANCEL_: u16 = 2;
const TIMER: usize = 1;

/// 一覧の 1 行が指すもの。
#[derive(Clone)]
enum Row {
    Active(u64),
    Done(DownloadRecord),
}

struct State {
    rows: Vec<Row>,
}

/// ダウンロードの一覧（進行中と履歴）。開いている間は 0.5 秒ごとに出し直す。
pub(super) fn show(owner: HWND) {
    let mut t = Template::dialog("ダウンロード", 440, 250);
    t.item(
        (WS_BORDER | WS_TABSTOP | WS_VSCROLL | WS_HSCROLL).0
            | (LBS_NOTIFY | LBS_NOINTEGRALHEIGHT) as u32,
        7,
        7,
        354,
        180,
        D_LIST,
        0x0083,
        "",
    );
    let mut y = 7;
    for (id, text) in [
        (D_OPEN, "開く"),
        (D_FOLDER, "フォルダを開く"),
        (D_PAUSE, "一時停止・再開"),
        (D_CANCEL, "中止"),
        (D_REMOVE, "履歴から消す"),
        (D_CLEAR, "履歴をすべて消す"),
    ] {
        t.item(
            WS_TABSTOP.0 | BS_PUSHBUTTON as u32,
            367,
            y,
            66,
            14,
            id,
            CLASS_BUTTON,
            text,
        );
        y += 17;
    }
    t.item(0, 7, 195, 40, 10, 0xFFFF, CLASS_STATIC, "保存先");
    t.item(
        (WS_BORDER).0 | (ES_AUTOHSCROLL | ES_READONLY) as u32,
        50,
        193,
        311,
        13,
        D_DIR,
        CLASS_EDIT,
        "",
    );
    t.item(
        WS_TABSTOP.0 | BS_PUSHBUTTON as u32,
        367,
        192,
        66,
        14,
        D_DIR_CHANGE,
        CLASS_BUTTON,
        "変更...",
    );
    t.item(
        WS_TABSTOP.0 | BS_AUTOCHECKBOX as u32,
        50,
        211,
        311,
        12,
        D_ASK,
        CLASS_BUTTON,
        "ダウンロードのたびに保存先を確かめる",
    );
    t.item(
        WS_TABSTOP.0 | BS_DEFPUSHBUTTON as u32,
        383,
        229,
        50,
        14,
        IDCANCEL_,
        CLASS_BUTTON,
        "閉じる",
    );
    let aligned = t.aligned();
    let mut st = State { rows: Vec::new() };
    unsafe {
        DialogBoxIndirectParamW(
            None,
            aligned.as_ptr() as *const DLGTEMPLATE,
            Some(owner),
            Some(dialog_proc),
            LPARAM(&mut st as *mut State as isize),
        );
    }
}

fn set_text(dlg: HWND, id: u16, s: &str) {
    unsafe {
        let _ = SetDlgItemTextW(dlg, id as i32, &HSTRING::from(s));
    }
}

fn selected(dlg: HWND) -> Option<usize> {
    let i =
        unsafe { SendDlgItemMessageW(dlg, D_LIST as i32, LB_GETCURSEL, WPARAM(0), LPARAM(0)).0 };
    (i >= 0).then_some(i as usize)
}

fn ago(t: u64) -> String {
    let secs = yy_adblock::lists::now().saturating_sub(t);
    match secs {
        0..60 => "今".into(),
        60..3600 => format!("{} 分前", secs / 60),
        3600..86400 => format!("{} 時間前", secs / 3600),
        _ => format!("{} 日前", secs / 86400),
    }
}

/// 一覧を作り直す（選んでいた行はなるべく残す）。
fn fill(dlg: HWND, st: &mut State) {
    let keep = selected(dlg);
    let Some((active, log, dir, ask)) = with(|a| {
        let active: Vec<(u64, String)> = a
            .downloads
            .iter()
            .map(|d| {
                (
                    d.id,
                    format!(
                        "{} {}　—　{}　—　{}",
                        if d.paused { "⏸" } else { "⏬" },
                        d.name(),
                        d.progress(),
                        d.url
                    ),
                )
            })
            .collect();
        let log: Vec<DownloadRecord> = history::load(&log_path(a));
        (
            active,
            log,
            a.profiles.download_dir.clone(),
            a.profiles.download_ask,
        )
    }) else {
        return;
    };
    let mut lines = Vec::new();
    st.rows.clear();
    for (id, s) in active {
        st.rows.push(Row::Active(id));
        lines.push(s);
    }
    for r in log.into_iter().rev() {
        let name = Path::new(&r.path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| r.path.clone());
        let mark = match r.state {
            DownloadState::Completed => "✔",
            DownloadState::Cancelled => "✖",
            DownloadState::Failed => "⚠",
            DownloadState::Duplicate => "♻",
        };
        lines.push(format!(
            "{mark} {name}　—　{} {}・{}　—　{}",
            r.state.label(),
            crate::util::human_size(r.bytes),
            ago(r.time),
            r.url
        ));
        st.rows.push(Row::Done(r));
    }
    unsafe {
        SendDlgItemMessageW(dlg, D_LIST as i32, WM_SETREDRAW, WPARAM(0), LPARAM(0));
        SendDlgItemMessageW(dlg, D_LIST as i32, LB_RESETCONTENT, WPARAM(0), LPARAM(0));
        let mut widest = 0;
        for l in &lines {
            widest = widest.max(l.chars().count());
            let s = HSTRING::from(l.as_str());
            SendDlgItemMessageW(
                dlg,
                D_LIST as i32,
                LB_ADDSTRING,
                WPARAM(0),
                LPARAM(s.as_ptr() as isize),
            );
        }
        SendDlgItemMessageW(
            dlg,
            D_LIST as i32,
            LB_SETHORIZONTALEXTENT,
            WPARAM(widest * 14),
            LPARAM(0),
        );
        if let Some(i) = keep.filter(|i| *i < lines.len()) {
            SendDlgItemMessageW(dlg, D_LIST as i32, LB_SETCURSEL, WPARAM(i), LPARAM(0));
        }
        SendDlgItemMessageW(dlg, D_LIST as i32, WM_SETREDRAW, WPARAM(1), LPARAM(0));
        let _ = windows::Win32::Graphics::Gdi::InvalidateRect(
            Some(GetDlgItem(Some(dlg), D_LIST as i32).unwrap_or_default()),
            None,
            true,
        );
    }
    set_text(
        dlg,
        D_DIR,
        if dir.trim().is_empty() {
            "（Windows の「ダウンロード」フォルダ）"
        } else {
            &dir
        },
    );
    unsafe {
        let _ = CheckDlgButton(
            dlg,
            D_ASK as i32,
            if ask { BST_CHECKED } else { BST_UNCHECKED },
        );
    }
}

/// 進行中のダウンロードの操作を探す。
fn active_op(id: u64) -> Option<(ICoreWebView2DownloadOperation, bool)> {
    with(|a| {
        a.downloads
            .iter()
            .find(|d| d.id == id)
            .map(|d| (d.op.clone(), d.paused))
    })
    .flatten()
}

/// 設定（保存先・尋ねるか）を変えて保存する。
fn save_settings(dlg: HWND, f: impl FnOnce(&mut yy_browser::ProfileList)) {
    let Some(list) = with(|a| {
        f(&mut a.profiles);
        a.profiles.clone()
    }) else {
        return;
    };
    if let Err(e) = list.save(&super::profiles_path()) {
        crate::util::error_box(dlg, &format!("保存できません: {e}"));
    }
}

extern "system" fn dialog_proc(dlg: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> isize {
    unsafe {
        match msg {
            WM_INITDIALOG => {
                SetWindowLongPtrW(dlg, GWLP_USERDATA, lparam.0);
                let st = &mut *(lparam.0 as *mut State);
                fill(dlg, st);
                SendDlgItemMessageW(dlg, D_LIST as i32, LB_SETCURSEL, WPARAM(0), LPARAM(0));
                SetTimer(Some(dlg), TIMER, 500, None);
                1
            }
            WM_TIMER => {
                let st = &mut *(GetWindowLongPtrW(dlg, GWLP_USERDATA) as *mut State);
                // 進行中があるときだけ出し直す（選んでいる行が動かないように）
                if with(|a| !a.downloads.is_empty()).unwrap_or(false)
                    || st.rows.iter().any(|r| matches!(r, Row::Active(_)))
                {
                    fill(dlg, st);
                }
                0
            }
            WM_COMMAND => {
                let st = &mut *(GetWindowLongPtrW(dlg, GWLP_USERDATA) as *mut State);
                let id = (wparam.0 & 0xffff) as u16;
                let code = ((wparam.0 >> 16) & 0xffff) as u32;
                let row = selected(dlg).and_then(|i| st.rows.get(i).cloned());
                match id {
                    D_OPEN => match &row {
                        Some(Row::Done(r))
                            if matches!(
                                r.state,
                                DownloadState::Completed | DownloadState::Duplicate
                            ) =>
                        {
                            open_path(&r.path)
                        }
                        Some(Row::Done(_)) => {
                            crate::util::info_box(dlg, "終わっていないダウンロードは開けません。")
                        }
                        Some(Row::Active(_)) => {
                            crate::util::info_box(dlg, "ダウンロードが終わってから開いてください。")
                        }
                        None => {}
                    },
                    D_LIST if code == LBN_DBLCLK => {
                        if let Some(Row::Done(r)) = &row
                            && matches!(
                                r.state,
                                DownloadState::Completed | DownloadState::Duplicate
                            )
                        {
                            open_path(&r.path);
                        }
                    }
                    D_FOLDER => match &row {
                        Some(Row::Done(r)) => reveal(&r.path),
                        Some(Row::Active(id)) => {
                            if let Some(p) = with(|a| {
                                a.downloads
                                    .iter()
                                    .find(|d| d.id == *id)
                                    .map(|d| d.path.clone())
                            })
                            .flatten()
                                && let Some(dir) = Path::new(&p).parent()
                            {
                                open_path(&dir.to_string_lossy());
                            }
                        }
                        None => {}
                    },
                    D_PAUSE => {
                        if let Some(Row::Active(id)) = &row
                            && let Some((op, paused)) = active_op(*id)
                        {
                            if paused {
                                let mut can = windows::core::BOOL(0);
                                let _ = op.CanResume(&mut can);
                                if can.as_bool() {
                                    let _ = op.Resume();
                                    with(|a| {
                                        if let Some(d) =
                                            a.downloads.iter_mut().find(|d| d.id == *id)
                                        {
                                            d.paused = false;
                                        }
                                    });
                                } else {
                                    crate::util::info_box(
                                        dlg,
                                        "このダウンロードは再開できません。",
                                    );
                                }
                            } else {
                                let _ = op.Pause();
                                with(|a| {
                                    if let Some(d) = a.downloads.iter_mut().find(|d| d.id == *id) {
                                        d.paused = true;
                                    }
                                });
                            }
                            fill(dlg, st);
                        }
                    }
                    D_CANCEL => {
                        if let Some(Row::Active(id)) = &row
                            && let Some((op, _)) = active_op(*id)
                        {
                            with(|a| {
                                a.cancelled.push(*id);
                                if let Some(d) = a.downloads.iter_mut().find(|d| d.id == *id) {
                                    d.paused = false;
                                }
                            });
                            let _ = op.Cancel();
                            fill(dlg, st);
                        }
                    }
                    D_REMOVE => {
                        if let Some(Row::Done(r)) = &row {
                            with(|a| {
                                let p = log_path(a);
                                let mut all: Vec<DownloadRecord> = history::load(&p);
                                if let Some(i) = all.iter().position(|x| x == r) {
                                    all.remove(i);
                                    let _ = history::save(&p, &all);
                                }
                            });
                            fill(dlg, st);
                        }
                    }
                    D_CLEAR => {
                        let ok = MessageBoxW(
                            Some(dlg),
                            w!(
                                "ダウンロードの履歴をすべて消しますか？\n（ダウンロードしたファイルは消しません）"
                            ),
                            w!("yybrowser"),
                            MB_OKCANCEL | MB_ICONQUESTION,
                        ) == IDOK;
                        if ok {
                            with(|a| history::clear(&log_path(a)));
                            fill(dlg, st);
                        }
                    }
                    D_DIR_CHANGE => {
                        if let Some(p) = pick_folder(dlg) {
                            save_settings(dlg, |l| {
                                l.download_dir = p.to_string_lossy().into_owned()
                            });
                            fill(dlg, st);
                        }
                    }
                    D_ASK => {
                        let on = IsDlgButtonChecked(dlg, D_ASK as i32) == BST_CHECKED.0;
                        save_settings(dlg, |l| l.download_ask = on);
                    }
                    IDCANCEL_ => {
                        let _ = KillTimer(Some(dlg), TIMER);
                        let _ = EndDialog(dlg, IDCANCEL_ as isize);
                    }
                    _ => {}
                }
                1
            }
            _ => 0,
        }
    }
}

/// フォルダを選ぶ。
fn pick_folder(owner: HWND) -> Option<PathBuf> {
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree};
    use windows::Win32::UI::Shell::{
        FOS_PICKFOLDERS, FileOpenDialog, IFileOpenDialog, SIGDN_FILESYSPATH,
    };
    unsafe {
        let d: IFileOpenDialog =
            CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let opts = d.GetOptions().ok()?;
        let _ = d.SetOptions(opts | FOS_PICKFOLDERS);
        let _ = d.SetTitle(w!("ダウンロードの保存先"));
        d.Show(Some(owner)).ok()?;
        let item = d.GetResult().ok()?;
        let p = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s.map(PathBuf::from)
    }
}
