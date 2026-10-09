//! ファイル管理 yyfilemanager の画面（18 章 9）。
//!
//! タブ（同期・検索・似たファイル・重複・削除の確認・記録）ごとに、上に条件の欄とボタン、下に一覧
//! （仮想リスト。数十万行でも軽い）を置く。走査・ハッシュ・同期・削除などの長い処理は作業スレッド
//! （[`work`]）で 1 つずつ行い、進みはステータスバーに出す（Esc で中止）。中核は `yy-files`。

mod work;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::InvalidateRect;
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PCWSTR, PWSTR, Result, w};
use yy_config::Config;
use yy_files::dupes::{FileRef, Group};
use yy_files::jobs::{Dirs, JobList, SavedSearch, SearchList, SyncJob};
use yy_files::purge::{Candidate, Review};
use yy_files::scan::Catalog;
use yy_files::similar::VersionGroup;
use yy_files::sync::{Action, Compare, ItemState, Mode, Plan, Run, SyncOptions};

use crate::util::{Context, error_box, info_box};
use crate::{default_proc, hiword, loword};
use work::{Msg, Task};

const FRAME_CLASS: PCWSTR = w!("YYFileManagerFrame");

const SS_CENTERIMAGE: u32 = 0x200;

/// 作業スレッドからの知らせ
const WM_APP_FM: u32 = WM_APP + 90;

// タブ
const TAB_SYNC: usize = 0;
const TAB_SEARCH: usize = 1;
const TAB_SIMILAR: usize = 2;
const TAB_DUPES: usize = 3;
const TAB_REVIEW: usize = 4;
const TAB_LOG: usize = 5;
const TAB_NAMES: [&str; 6] = ["同期", "検索", "似たファイル", "重複", "削除の確認", "記録"];

// 部品の ID
const ID_TABS: u16 = 5000;
const ID_STATUS: u16 = 5001;
const ID_LOG: u16 = 5002;
const ID_LIST_BASE: u16 = 5010;
// 同期
const ID_JOB: u16 = 5100;
const ID_JOB_SAVE: u16 = 5101;
const ID_JOB_DELETE: u16 = 5102;
const ID_MODE: u16 = 5103;
const ID_COMPARE: u16 = 5104;
const ID_SRC: u16 = 5105;
const ID_SRC_BROWSE: u16 = 5106;
const ID_DST: u16 = 5107;
const ID_DST_BROWSE: u16 = 5108;
const ID_PLAN: u16 = 5109;
const ID_RUN: u16 = 5110;
const ID_RESUME: u16 = 5111;
const ID_SWAP: u16 = 5112;
// 検索
const ID_QUERY: u16 = 5200;
const ID_SEARCH: u16 = 5201;
const ID_SEARCH_ROOTS: u16 = 5202;
const ID_SEARCH_BROWSE: u16 = 5203;
const ID_OFFICE: u16 = 5204;
const ID_SAVED: u16 = 5205;
const ID_SAVED_SAVE: u16 = 5206;
const ID_SAVED_DELETE: u16 = 5207;
const ID_RESCAN: u16 = 5208;
// 似たファイル・重複
const ID_SIM_ROOTS: u16 = 5300;
const ID_SIM_BROWSE: u16 = 5301;
const ID_SIM_FIND: u16 = 5302;
const ID_SIM_TO_REVIEW: u16 = 5303;
const ID_SIM_EXTS: u16 = 5304;
const ID_SIM_REGEX: u16 = 5305;
const ID_DUP_ROOTS: u16 = 5400;
const ID_DUP_BROWSE: u16 = 5401;
const ID_DUP_FIND: u16 = 5402;
const ID_DUP_TO_REVIEW: u16 = 5403;
const ID_DUP_EXTS: u16 = 5404;
const ID_DUP_REGEX: u16 = 5405;
// 削除の確認
const ID_CHECK_ALL: u16 = 5500;
const ID_UNCHECK_ALL: u16 = 5501;
const ID_PURGE: u16 = 5502;
const ID_UNDO: u16 = 5503;
const ID_CLEAR_REVIEW: u16 = 5504;
// 共通
const ID_CANCEL: u16 = 5600;
// メニュー
const ID_EXIT: u16 = 5700;
const ID_TAB_BASE: u16 = 5710;
const ID_HELP: u16 = 5720;
const ID_ABOUT: u16 = 5721;
const ID_OPEN_LOGS: u16 = 5722;
const ID_CRASH_LOGS: u16 = 5723;
const ID_SETTINGS: u16 = 5724;
const ID_EXPIRE_TRASH: u16 = 5725;
const ID_VACUUM: u16 = 5726;
// 一覧の右クリック
const CM_OPEN: u32 = 1;
const CM_LOCATE: u32 = 2;
const CM_EDITOR: u32 = 3;
const CM_COPY_PATH: u32 = 4;
const CM_TO_REVIEW: u32 = 5;
const CM_EXPORT: u32 = 6;
const CM_OVERWRITE: u32 = 7;
const CM_KEEP_BOTH: u32 = 8;
const CM_SKIP: u32 = 9;
const CM_CHECK: u32 = 10;
const CM_UNCHECK: u32 = 11;
const CM_REMOVE: u32 = 12;
const CM_VERSIONS: u32 = 13;
const CM_SAME_CONTENT: u32 = 14;
const CM_SYNC_SRC: u32 = 15;

/// 一覧の列（見出し・幅・右寄せ）。
const COLUMNS: [&[(&str, i32, bool)]; 5] = [
    &[
        ("種類", 70, false),
        ("パス", 360, false),
        ("大きさ", 80, true),
        ("送り元の日時", 130, false),
        ("送り先の日時", 130, false),
        ("理由", 220, false),
        ("状態", 140, false),
    ],
    &[
        ("名前", 220, false),
        ("フォルダ", 300, false),
        ("大きさ", 80, true),
        ("更新日時", 130, false),
        ("行", 50, true),
        ("一致した行", 360, false),
    ],
    &[
        ("グループ", 60, true),
        ("判定", 90, false),
        ("名前", 240, false),
        ("フォルダ", 260, false),
        ("大きさ", 80, true),
        ("更新日時", 130, false),
        ("自信", 40, false),
        ("理由", 320, false),
    ],
    &[
        ("グループ", 60, true),
        ("判定", 60, false),
        ("パス", 420, false),
        ("大きさ", 80, true),
        ("更新日時", 130, false),
    ],
    &[
        ("判定", 60, false),
        ("パス", 420, false),
        ("大きさ", 80, true),
        ("更新日時", 130, false),
        ("理由", 160, false),
        ("自信", 40, false),
        ("グループ", 60, true),
    ],
];

/// 部品の幅（96 DPI のピクセル。0 は残りの幅）。
#[derive(Clone, Copy)]
struct Cell {
    hwnd: HWND,
    width: i32,
}

/// 検索の結果の 1 行。
struct SearchRow {
    file: FileRef,
    line: u64,
    text: String,
}

/// 同期のタブの一覧の 1 行（計画か実行）。
#[derive(Clone)]
struct SyncRow {
    item: yy_files::sync::Item,
    state: String,
}

struct App {
    frame: HWND,
    tabs: HWND,
    status: HWND,
    log: HWND,
    dpi: u32,
    config: Config,
    dirs: Dirs,
    /// タブごとの部品の行と一覧
    rows: Vec<Vec<Vec<Cell>>>,
    lists: Vec<HWND>,
    current: usize,
    edits: Edits,
    // 同期
    jobs: JobList,
    plan: Option<Plan>,
    sync_rows: Vec<SyncRow>,
    /// 続けられる実行（ジャーナルのパス・実行）
    resume: Option<(PathBuf, Run)>,
    // 検索
    searches: SearchList,
    search_cats: Vec<Catalog>,
    search_rows: Vec<SearchRow>,
    // 似たファイル
    sim_cats: Vec<Catalog>,
    sim_groups: Vec<VersionGroup>,
    sim_rows: Vec<(usize, usize)>,
    // 重複
    dup_cats: Vec<Catalog>,
    dup_groups: Vec<Group>,
    dup_rows: Vec<(usize, usize)>,
    // 削除の確認
    review: Review,
    /// 実行中の処理（中止の印・説明）
    busy: Option<(Arc<AtomicBool>, String)>,
    rx: mpsc::Receiver<Msg>,
    notify: Arc<work::Notify>,
    log_text: String,
}

/// 入力の欄。
#[derive(Default)]
struct Edits {
    job: HWND,
    mode: HWND,
    compare: HWND,
    src: HWND,
    dst: HWND,
    sync_info: HWND,
    query: HWND,
    search_roots: HWND,
    office: HWND,
    saved: HWND,
    rescan: HWND,
    sim_roots: HWND,
    sim_exts: HWND,
    sim_regex: HWND,
    dup_roots: HWND,
    dup_exts: HWND,
    dup_regex: HWND,
    review_info: HWND,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| a.try_borrow_mut().ok()?.as_mut().map(f))
}

fn text_of(h: HWND) -> String {
    unsafe {
        let n = GetWindowTextLengthW(h).max(0) as usize;
        let mut buf = vec![0u16; n + 1];
        let got = GetWindowTextW(h, &mut buf) as usize;
        String::from_utf16_lossy(&buf[..got])
    }
}

fn set_text(h: HWND, s: &str) {
    unsafe {
        let _ = SetWindowTextW(h, &HSTRING::from(s));
    }
}

/// `;` 区切りの場所。
fn roots_of(s: &str) -> Vec<PathBuf> {
    s.split(';')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .collect()
}

/// 日時（手元の時刻）。
fn time_text(ns: i64) -> String {
    if ns == 0 {
        return String::new();
    }
    // FILETIME（1601 年からの 100 ns）にしてから手元の時刻へ
    let ft = (ns / 100 + 116_444_736_000_000_000) as u64;
    let f = windows::Win32::Foundation::FILETIME {
        dwLowDateTime: ft as u32,
        dwHighDateTime: (ft >> 32) as u32,
    };
    unsafe {
        let mut utc = windows::Win32::Foundation::SYSTEMTIME::default();
        let mut local = windows::Win32::Foundation::SYSTEMTIME::default();
        if windows::Win32::System::Time::FileTimeToSystemTime(&f, &mut utc).is_err()
            || windows::Win32::System::Time::SystemTimeToTzSpecificLocalTime(None, &utc, &mut local)
                .is_err()
        {
            return String::new();
        }
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}",
            local.wYear, local.wMonth, local.wDay, local.wHour, local.wMinute
        )
    }
}

/// UTC との差（秒）。
fn tz_offset() -> i64 {
    let mut tz = windows::Win32::System::Time::TIME_ZONE_INFORMATION::default();
    let r = unsafe { windows::Win32::System::Time::GetTimeZoneInformation(&mut tz) };
    let dst = if r == 2 { tz.DaylightBias } else { 0 };
    -((tz.Bias + dst) as i64) * 60
}

fn now_ns() -> i64 {
    yy_files::fs::to_nanos(std::time::SystemTime::now())
}

/// 日時の印（手元の時刻。`2026-10-08 1530`）。
fn local_stamp() -> String {
    yy_files::sync::stamp(now_ns() + tz_offset() * 1_000_000_000)
}

/// 共有フォルダ（UNC・ネットワーク ドライブ）か。
fn is_network(p: &Path) -> bool {
    let s = p.to_string_lossy();
    if s.starts_with(r"\\") && !s.starts_with(r"\\?\") || s.starts_with(r"\\?\UNC\") {
        return true;
    }
    let b = s.as_bytes();
    if b.len() >= 2 && b[1] == b':' {
        let root: Vec<u16> = format!("{}:\\", b[0] as char)
            .encode_utf16()
            .chain([0])
            .collect();
        // DRIVE_REMOTE
        return unsafe {
            windows::Win32::Storage::FileSystem::GetDriveTypeW(PCWSTR(root.as_ptr()))
        } == 4;
    }
    false
}

/// ごみ箱へ送る。
fn recycle(path: &Path) -> std::io::Result<()> {
    use windows::Win32::UI::Shell::{
        FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, SHFILEOPSTRUCTW,
        SHFileOperationW,
    };
    let from: Vec<u16> = path
        .as_os_str()
        .to_string_lossy()
        .encode_utf16()
        .chain([0, 0])
        .collect();
    let mut op = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: PCWSTR(from.as_ptr()),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_SILENT).0 as u16,
        ..Default::default()
    };
    let r = unsafe { SHFileOperationW(&mut op) };
    if r != 0 || op.fAnyOperationsAborted.as_bool() {
        return Err(std::io::Error::other(format!(
            "ごみ箱へ送れませんでした（{r:#x}）"
        )));
    }
    Ok(())
}

/// yyfilemanager を起動する。`args` はコマンドライン（`--sync <名前>` なら画面を出さずに同期する）。
pub fn run_filemanager(args: Vec<String>) -> Result<()> {
    crate::util::set_app_name("yyfilemanager");
    if let Some(i) = args.iter().position(|a| a == "--sync") {
        let name = args.get(i + 1).cloned().unwrap_or_default();
        std::process::exit(work::headless_sync(&name));
    }
    crate::crash::install("yyfilemanager");
    let r = run_inner();
    crate::crash::clean_exit();
    if let Err(e) = &r {
        error_box(
            HWND::default(),
            &format!("起動できませんでした。\n{}", crate::util::describe_error(e)),
        );
    }
    r
}

fn run_inner() -> Result<()> {
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
            .ok()
            .context("CoInitializeEx")?;
        let icc = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_BAR_CLASSES | ICC_TAB_CLASSES | ICC_LISTVIEW_CLASSES | ICC_STANDARD_CLASSES,
        };
        let _ = InitCommonControlsEx(&icc);
    }
    if let Ok(m) = unsafe { GetModuleHandleW(None) } {
        crate::help::use_app_help(m.into(), crate::help::FM_MD, "yyfilemanager ヘルプ");
    }
    create()?;
    unsafe {
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if msg.message == WM_KEYDOWN && key_hook(&msg) {
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    APP.with(|a| a.borrow_mut().take());
    Ok(())
}

fn create_menu() -> Result<HMENU> {
    unsafe {
        let bar = CreateMenu()?;
        let add = |m: HMENU, id: u16, t: &str| {
            let _ = AppendMenuW(m, MF_STRING, id as usize, &HSTRING::from(t));
        };
        let file = CreatePopupMenu()?;
        add(file, ID_SETTINGS, "設定ファイルを開く(&S)");
        let _ = AppendMenuW(file, MF_SEPARATOR, 0, None);
        add(file, ID_EXIT, "終了(&X)");
        let view = CreatePopupMenu()?;
        for (i, t) in TAB_NAMES.iter().enumerate() {
            add(
                view,
                ID_TAB_BASE + i as u16,
                &format!("{t}(&{})\tCtrl+{}", i + 1, i + 1),
            );
        }
        let tidy = CreatePopupMenu()?;
        add(tidy, ID_EXPIRE_TRASH, "隔離フォルダの古いものを消す(&T)...");
        add(tidy, ID_VACUUM, "中身の索引のバキューム(&V)...");
        let help = CreatePopupMenu()?;
        add(help, ID_HELP, "yyfilemanager ヘルプ(&H)\tF1");
        let _ = AppendMenuW(help, MF_SEPARATOR, 0, None);
        add(help, ID_OPEN_LOGS, "記録のフォルダを開く(&L)");
        add(help, ID_CRASH_LOGS, "異常終了の記録のフォルダを開く(&C)");
        add(help, ID_ABOUT, "yyfilemanager について(&A)");
        for (m, t) in [
            (file, "ファイル(&F)"),
            (view, "表示(&V)"),
            (tidy, "整理(&O)"),
            (help, "ヘルプ(&H)"),
        ] {
            AppendMenuW(bar, MF_POPUP, m.0 as usize, &HSTRING::from(t))?;
        }
        Ok(bar)
    }
}

fn create() -> Result<HWND> {
    unsafe {
        let instance: windows::Win32::Foundation::HINSTANCE = GetModuleHandleW(None)?.into();
        let (icon, icon_small) = crate::app_icons(instance);
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(frame_proc),
            hInstance: instance,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            hbrBackground: windows::Win32::Graphics::Gdi::HBRUSH(
                (windows::Win32::Graphics::Gdi::COLOR_BTNFACE.0 + 1) as usize as *mut _,
            ),
            hIcon: icon,
            hIconSm: icon_small,
            lpszClassName: FRAME_CLASS,
            ..Default::default()
        };
        if RegisterClassExW(&class) == 0 {
            return Err(windows::core::Error::from_thread());
        }
        let (config, config_error) = Config::load();
        let frame = CreateWindowExW(
            WS_EX_ACCEPTFILES,
            FRAME_CLASS,
            w!("yyfilemanager"),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1200,
            800,
            None,
            Some(create_menu()?),
            Some(instance),
            None,
        )
        .context("CreateWindowExW(frame)")?;
        let dpi = GetDpiForWindow(frame).max(96);
        let font = crate::util::ui_font(dpi);
        let child =
            |class: PCWSTR, text: &str, style: WINDOW_STYLE, ex: WINDOW_EX_STYLE, id: u16| {
                let h = CreateWindowExW(
                    ex,
                    class,
                    &HSTRING::from(text),
                    WS_CHILD | style,
                    0,
                    0,
                    10,
                    10,
                    Some(frame),
                    Some(HMENU(id as usize as *mut _)),
                    Some(instance),
                    None,
                )
                .unwrap_or_default();
                SendMessageW(
                    h,
                    WM_SETFONT,
                    Some(WPARAM(font.0 as usize)),
                    Some(LPARAM(1)),
                );
                h
            };
        let tabs = child(
            WC_TABCONTROLW,
            "",
            WS_VISIBLE | WS_CLIPSIBLINGS | WINDOW_STYLE(TCS_FOCUSNEVER),
            WINDOW_EX_STYLE(0),
            ID_TABS,
        );
        for (i, t) in TAB_NAMES.iter().enumerate() {
            let s = crate::util::wide(t);
            let item = TCITEMW {
                mask: TCIF_TEXT,
                pszText: PWSTR(s.as_ptr() as *mut _),
                ..Default::default()
            };
            SendMessageW(
                tabs,
                TCM_INSERTITEMW,
                Some(WPARAM(i)),
                Some(LPARAM(&item as *const _ as isize)),
            );
        }
        let status = child(
            STATUSCLASSNAMEW,
            "",
            WS_VISIBLE | WINDOW_STYLE(SBARS_SIZEGRIP),
            WINDOW_EX_STYLE(0),
            ID_STATUS,
        );
        let label = |t: &str| {
            child(
                w!("STATIC"),
                t,
                WINDOW_STYLE(SS_CENTERIMAGE),
                WINDOW_EX_STYLE(0),
                0,
            )
        };
        let button = |t: &str, id: u16| {
            child(
                w!("BUTTON"),
                t,
                WS_TABSTOP | WINDOW_STYLE(BS_PUSHBUTTON as u32),
                WINDOW_EX_STYLE(0),
                id,
            )
        };
        let edit = |id: u16| {
            child(
                w!("EDIT"),
                "",
                WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
                WS_EX_CLIENTEDGE,
                id,
            )
        };
        let combo = |id: u16, editable: bool| {
            let style = if editable {
                CBS_DROPDOWN
            } else {
                CBS_DROPDOWNLIST
            };
            child(
                w!("COMBOBOX"),
                "",
                WS_TABSTOP | WS_VSCROLL | WINDOW_STYLE((style | CBS_AUTOHSCROLL) as u32),
                WINDOW_EX_STYLE(0),
                id,
            )
        };
        let check = |t: &str, id: u16| {
            child(
                w!("BUTTON"),
                t,
                WS_TABSTOP | WINDOW_STYLE(BS_AUTOCHECKBOX as u32),
                WINDOW_EX_STYLE(0),
                id,
            )
        };
        let c = |hwnd: HWND, width: i32| Cell { hwnd, width };
        // 同期
        let mut e = Edits {
            job: combo(ID_JOB, true),
            mode: combo(ID_MODE, false),
            compare: combo(ID_COMPARE, false),
            ..Default::default()
        };
        for t in ["更新", "ミラー"] {
            SendMessageW(
                e.mode,
                CB_ADDSTRING,
                None,
                Some(LPARAM(HSTRING::from(t).as_ptr() as isize)),
            );
        }
        for t in ["大きさと日時", "中身で比べる"] {
            SendMessageW(
                e.compare,
                CB_ADDSTRING,
                None,
                Some(LPARAM(HSTRING::from(t).as_ptr() as isize)),
            );
        }
        SendMessageW(e.mode, CB_SETCURSEL, Some(WPARAM(0)), None);
        SendMessageW(e.compare, CB_SETCURSEL, Some(WPARAM(0)), None);
        e.src = edit(ID_SRC);
        e.dst = edit(ID_DST);
        e.sync_info = label("");
        let cancel = |_: ()| button("中止", ID_CANCEL);
        let sync_rows = vec![
            vec![
                c(label("同期ジョブ"), 70),
                c(e.job, 220),
                c(button("保存", ID_JOB_SAVE), 60),
                c(button("削除", ID_JOB_DELETE), 60),
                c(label("やり方"), 50),
                c(e.mode, 100),
                c(label("比べ方"), 50),
                c(e.compare, 130),
            ],
            vec![
                c(label("送り元"), 70),
                c(e.src, 0),
                c(button("参照...", ID_SRC_BROWSE), 70),
            ],
            vec![
                c(label("送り先"), 70),
                c(e.dst, 0),
                c(button("参照...", ID_DST_BROWSE), 70),
                c(button("⇅ 入れ替え", ID_SWAP), 90),
            ],
            vec![
                c(button("比べる", ID_PLAN), 80),
                c(button("実行", ID_RUN), 80),
                c(button("続ける", ID_RESUME), 80),
                c(cancel(()), 70),
                c(e.sync_info, 0),
            ],
        ];
        // 検索
        e.query = edit(ID_QUERY);
        e.search_roots = edit(ID_SEARCH_ROOTS);
        e.office = check("Office・PDF の中も", ID_OFFICE);
        SendMessageW(e.office, BM_SETCHECK, Some(WPARAM(1)), None);
        e.saved = combo(ID_SAVED, true);
        e.rescan = check("走査し直す", ID_RESCAN);
        let search_rows = vec![
            vec![
                c(label("検索"), 50),
                c(e.query, 0),
                c(button("検索", ID_SEARCH), 70),
                c(cancel(()), 70),
            ],
            vec![
                c(label("場所"), 50),
                c(e.search_roots, 0),
                c(button("追加...", ID_SEARCH_BROWSE), 70),
                c(e.office, 150),
            ],
            vec![
                c(label("保存した検索"), 90),
                c(e.saved, 240),
                c(button("保存", ID_SAVED_SAVE), 60),
                c(button("削除", ID_SAVED_DELETE), 60),
                c(e.rescan, 110),
            ],
        ];
        e.sim_roots = edit(ID_SIM_ROOTS);
        e.sim_exts = edit(ID_SIM_EXTS);
        e.sim_regex = edit(ID_SIM_REGEX);
        let sim_rows = vec![
            vec![
                c(label("場所"), 50),
                c(e.sim_roots, 0),
                c(button("追加...", ID_SIM_BROWSE), 70),
                c(button("探す", ID_SIM_FIND), 70),
                c(cancel(()), 70),
                c(button("古い版を削除の確認へ", ID_SIM_TO_REVIEW), 170),
            ],
            vec![
                c(label("拡張子"), 50),
                c(e.sim_exts, 180),
                c(label("名前の正規表現"), 110),
                c(e.sim_regex, 0),
            ],
        ];
        e.dup_roots = edit(ID_DUP_ROOTS);
        e.dup_exts = edit(ID_DUP_EXTS);
        e.dup_regex = edit(ID_DUP_REGEX);
        let dup_rows = vec![
            vec![
                c(label("場所"), 50),
                c(e.dup_roots, 0),
                c(button("追加...", ID_DUP_BROWSE), 70),
                c(button("探す", ID_DUP_FIND), 70),
                c(cancel(()), 70),
                c(button("写しを削除の確認へ", ID_DUP_TO_REVIEW), 160),
            ],
            vec![
                c(label("拡張子"), 50),
                c(e.dup_exts, 180),
                c(label("名前の正規表現"), 110),
                c(e.dup_regex, 0),
            ],
        ];
        e.review_info = label("");
        let review_rows = vec![vec![
            c(button("すべてチェック", ID_CHECK_ALL), 110),
            c(button("チェックを外す", ID_UNCHECK_ALL), 110),
            c(button("削除...", ID_PURGE), 80),
            c(button("記録から元に戻す...", ID_UNDO), 150),
            c(button("一覧を空にする", ID_CLEAR_REVIEW), 120),
            c(e.review_info, 0),
        ]];
        let rows = vec![
            sync_rows,
            search_rows,
            sim_rows,
            dup_rows,
            review_rows,
            Vec::new(),
        ];
        let mut lists = Vec::new();
        for (i, cols) in COLUMNS.iter().enumerate() {
            let style = LVS_REPORT | LVS_SHOWSELALWAYS | LVS_OWNERDATA;
            let list = child(
                WC_LISTVIEWW,
                "",
                WS_TABSTOP | WINDOW_STYLE(style),
                WS_EX_CLIENTEDGE,
                ID_LIST_BASE + i as u16,
            );
            let _ = SetWindowTheme(list, w!("Explorer"), None);
            let mut ex = LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER | LVS_EX_LABELTIP;
            if i == TAB_REVIEW {
                ex |= LVS_EX_CHECKBOXES;
            }
            SendMessageW(
                list,
                LVM_SETEXTENDEDLISTVIEWSTYLE,
                Some(WPARAM(0)),
                Some(LPARAM(ex as isize)),
            );
            for (k, (t, wdt, right)) in cols.iter().enumerate() {
                let s = crate::util::wide(t);
                let col = LVCOLUMNW {
                    mask: LVCF_TEXT | LVCF_WIDTH | LVCF_FMT,
                    fmt: if *right { LVCFMT_RIGHT } else { LVCFMT_LEFT },
                    cx: wdt * dpi as i32 / 96,
                    pszText: PWSTR(s.as_ptr() as *mut _),
                    ..Default::default()
                };
                SendMessageW(
                    list,
                    LVM_INSERTCOLUMNW,
                    Some(WPARAM(k)),
                    Some(LPARAM(&col as *const _ as isize)),
                );
            }
            lists.push(list);
        }
        let log = child(
            w!("EDIT"),
            "",
            WS_VSCROLL
                | WS_HSCROLL
                | WINDOW_STYLE(
                    (ES_MULTILINE | ES_READONLY | ES_AUTOVSCROLL | ES_AUTOHSCROLL) as u32,
                ),
            WS_EX_CLIENTEDGE,
            ID_LOG,
        );
        SendMessageW(log, EM_SETLIMITTEXT, Some(WPARAM(0)), None);
        let dirs = work::dirs();
        let jobs = JobList::load(&dirs.jobs_file()).unwrap_or_default();
        let searches = SearchList::load(&dirs.searches_file()).unwrap_or_default();
        for s in &searches.searches {
            SendMessageW(
                e.saved,
                CB_ADDSTRING,
                None,
                Some(LPARAM(HSTRING::from(s.name.as_str()).as_ptr() as isize)),
            );
        }
        for j in &jobs.jobs {
            SendMessageW(
                e.job,
                CB_ADDSTRING,
                None,
                Some(LPARAM(HSTRING::from(j.name.as_str()).as_ptr() as isize)),
            );
        }
        let (tx, rx) = mpsc::channel();
        let notify = Arc::new(work::Notify::new(frame, tx));
        let resume = dirs.unfinished_runs().into_iter().next();
        let review = Review::load(&dirs.reviews().join("current.review")).unwrap_or_default();
        let app = App {
            frame,
            tabs,
            status,
            log,
            dpi,
            config,
            dirs,
            rows,
            lists,
            current: 0,
            edits: e,
            jobs,
            plan: None,
            sync_rows: Vec::new(),
            resume,
            searches,
            search_cats: Vec::new(),
            search_rows: Vec::new(),
            sim_cats: Vec::new(),
            sim_groups: Vec::new(),
            sim_rows: Vec::new(),
            dup_cats: Vec::new(),
            dup_groups: Vec::new(),
            dup_rows: Vec::new(),
            review,
            busy: None,
            rx,
            notify,
            log_text: String::new(),
        };
        APP.with(|a| *a.borrow_mut() = Some(app));
        with(|a| {
            a.show_tab(0);
            a.layout();
            a.log_line(&format!(
                "yyfilemanager {} を起動しました（置き場所: {}）",
                env!("CARGO_PKG_VERSION"),
                a.dirs.root.display()
            ));
            if let Some((_, r)) = a.resume.clone() {
                let c = r.counts();
                let msg = format!(
                    "前回の同期（{} → {}）が終わっていません（残り {} 件）。同期のタブの「続ける」で続けます。",
                    r.src_root.display(),
                    r.dst_root.display(),
                    c.pending
                );
                a.log_line(&msg);
                a.set_status(&msg);
                set_text(a.edits.src, &r.src_root.to_string_lossy());
                set_text(a.edits.dst, &r.dst_root.to_string_lossy());
                a.sync_rows = r
                    .items
                    .iter()
                    .map(|i| SyncRow {
                        item: i.item.clone(),
                        state: state_text(&i.state),
                    })
                    .collect();
                a.refresh_list(TAB_SYNC);
            }
            a.refresh_list(TAB_REVIEW);
            a.update_review_info();
        });
        let _ = ShowWindow(frame, SW_SHOWDEFAULT);
        let _ = windows::Win32::Graphics::Gdi::UpdateWindow(frame);
        if let Some(e) = config_error {
            error_box(
                frame,
                &format!("設定を読めませんでした（既定の設定で起動します）。\n{e}"),
            );
        }
        Ok(frame)
    }
}

fn state_text(s: &ItemState) -> String {
    match s {
        ItemState::Pending => String::new(),
        ItemState::Partial(n) => format!("送りかけ（{}）", yy_files::human_size(*n)),
        ItemState::Done => "済み".into(),
        ItemState::Failed(e) => format!("失敗: {e}"),
    }
}

impl App {
    fn set_status(&self, s: &str) {
        let w = HSTRING::from(s);
        unsafe {
            SendMessageW(
                self.status,
                SB_SETTEXTW,
                Some(WPARAM(0)),
                Some(LPARAM(w.as_ptr() as isize)),
            );
        }
    }

    /// 記録に書く（画面と `logs\filemanager.log`）。
    fn log_line(&mut self, s: &str) {
        let line = format!("{} {s}\r\n", crate::remote::local_clock());
        work::append_log_file(&line);
        self.log_text.push_str(&line);
        if self.log_text.len() > 4 << 20 {
            let cut = self.log_text.len() - (2 << 20);
            let cut = self.log_text[cut..].find('\n').map_or(cut, |i| cut + i + 1);
            self.log_text.drain(..cut);
        }
        set_text(self.log, &self.log_text);
        unsafe {
            let n = self.log_text.encode_utf16().count();
            SendMessageW(
                self.log,
                EM_SETSEL,
                Some(WPARAM(n)),
                Some(LPARAM(n as isize)),
            );
            SendMessageW(self.log, EM_SCROLLCARET, None, None);
        }
    }

    fn show_tab(&mut self, i: usize) {
        self.current = i;
        unsafe {
            SendMessageW(self.tabs, TCM_SETCURSEL, Some(WPARAM(i)), None);
            for (t, rows) in self.rows.iter().enumerate() {
                for row in rows {
                    for cell in row {
                        let _ = ShowWindow(cell.hwnd, if t == i { SW_SHOW } else { SW_HIDE });
                    }
                }
            }
            for (t, l) in self.lists.iter().enumerate() {
                let _ = ShowWindow(*l, if t == i { SW_SHOW } else { SW_HIDE });
            }
            let _ = ShowWindow(self.log, if i == TAB_LOG { SW_SHOW } else { SW_HIDE });
        }
        self.layout();
    }

    fn layout(&self) {
        unsafe {
            let mut rc = RECT::default();
            let _ = GetClientRect(self.frame, &mut rc);
            SendMessageW(self.status, WM_SIZE, None, None);
            let mut sr = RECT::default();
            let _ = GetWindowRect(self.status, &mut sr);
            let bottom = rc.bottom - (sr.bottom - sr.top);
            let d = |v: i32| v * self.dpi as i32 / 96;
            let _ = MoveWindow(self.tabs, 0, 0, rc.right, d(26), true);
            let mut y = d(30);
            let pad = d(6);
            let row_h = d(24);
            if let Some(rows) = self.rows.get(self.current) {
                for row in rows {
                    let fixed: i32 = row.iter().map(|c| d(c.width) + d(4)).sum();
                    let fills = row.iter().filter(|c| c.width == 0).count().max(1) as i32;
                    let fill_w = ((rc.right - 2 * pad - fixed) / fills).max(d(80));
                    let mut x = pad;
                    for c in row {
                        let w = if c.width == 0 { fill_w } else { d(c.width) };
                        // 組み合わせボックスは開いたときの高さも渡す
                        let h = if is_combo(c.hwnd) { d(240) } else { row_h };
                        let _ = MoveWindow(c.hwnd, x, y, w, h, true);
                        x += w + d(4);
                    }
                    y += row_h + d(4);
                }
            }
            let target = if self.current == TAB_LOG {
                self.log
            } else {
                self.lists[self.current]
            };
            let _ = MoveWindow(
                target,
                pad,
                y,
                rc.right - 2 * pad,
                (bottom - y - pad).max(0),
                true,
            );
        }
    }

    /// 一覧の行の数。
    fn row_count(&self, tab: usize) -> usize {
        match tab {
            TAB_SYNC => self.sync_rows.len(),
            TAB_SEARCH => self.search_rows.len(),
            TAB_SIMILAR => self.sim_rows.len(),
            TAB_DUPES => self.dup_rows.len(),
            TAB_REVIEW => self.review.items.len(),
            _ => 0,
        }
    }

    fn refresh_list(&self, tab: usize) {
        if let Some(l) = self.lists.get(tab) {
            unsafe {
                SendMessageW(
                    *l,
                    LVM_SETITEMCOUNT,
                    Some(WPARAM(self.row_count(tab))),
                    Some(LPARAM((LVSICF_NOSCROLL | LVSICF_NOINVALIDATEALL) as isize)),
                );
                let _ = InvalidateRect(Some(*l), None, false);
            }
        }
    }

    /// 一覧の文字列。
    fn cell(&self, tab: usize, row: usize, col: usize) -> String {
        let size = |n: u64| yy_files::human_size(n);
        match tab {
            TAB_SYNC => {
                let Some(r) = self.sync_rows.get(row) else {
                    return String::new();
                };
                let i = &r.item;
                match col {
                    0 => i.action.label().into(),
                    1 => i.rel.clone(),
                    2 => i.src.or(i.dst).map(|m| size(m.size)).unwrap_or_default(),
                    3 => i.src.map(|m| time_text(m.mtime)).unwrap_or_default(),
                    4 => i.dst.map(|m| time_text(m.mtime)).unwrap_or_default(),
                    5 => i.reason.clone(),
                    _ => r.state.clone(),
                }
            }
            TAB_SEARCH => {
                let Some(r) = self.search_rows.get(row) else {
                    return String::new();
                };
                let c = &self.search_cats[r.file.root];
                let f = &c.files[r.file.index];
                match col {
                    0 => f.name().into(),
                    1 => yy_files::join(&c.root, f.dir())
                        .to_string_lossy()
                        .into_owned(),
                    2 => size(f.meta.size),
                    3 => time_text(f.meta.mtime),
                    4 => {
                        if r.line > 0 {
                            r.line.to_string()
                        } else {
                            String::new()
                        }
                    }
                    _ => r.text.clone(),
                }
            }
            TAB_SIMILAR => {
                let Some(&(g, m)) = self.sim_rows.get(row) else {
                    return String::new();
                };
                let grp = &self.sim_groups[g];
                let mem = &grp.members[m];
                let c = &self.sim_cats[mem.file.root];
                let f = &c.files[mem.file.index];
                match col {
                    0 => (g + 1).to_string(),
                    1 => if m == 0 {
                        "最新（提案）"
                    } else {
                        "古い版"
                    }
                    .into(),
                    2 => f.name().into(),
                    3 => yy_files::join(&c.root, f.dir())
                        .to_string_lossy()
                        .into_owned(),
                    4 => size(f.meta.size),
                    5 => time_text(f.meta.mtime),
                    6 => if m == 0 { grp.confidence.label() } else { "" }.into(),
                    _ => {
                        if m == 0 {
                            grp.reason.clone()
                        } else {
                            String::new()
                        }
                    }
                }
            }
            TAB_DUPES => {
                let Some(&(g, k)) = self.dup_rows.get(row) else {
                    return String::new();
                };
                let grp = &self.dup_groups[g];
                let r = grp.files[k];
                let c = &self.dup_cats[r.root];
                let f = &c.files[r.index];
                match col {
                    0 => (g + 1).to_string(),
                    1 => if k == 0 { "残す" } else { "写し" }.into(),
                    2 => c.path(f).to_string_lossy().into_owned(),
                    3 => size(f.meta.size),
                    _ => time_text(f.meta.mtime),
                }
            }
            TAB_REVIEW => {
                let Some(c) = self.review.items.get(row) else {
                    return String::new();
                };
                match col {
                    0 => if c.keep { "残す" } else { "消す" }.into(),
                    1 => c.path().to_string_lossy().into_owned(),
                    2 => size(c.meta.size),
                    3 => time_text(c.meta.mtime),
                    4 => c.reason.clone(),
                    5 => c.confidence.clone(),
                    _ => (c.group + 1).to_string(),
                }
            }
            _ => String::new(),
        }
    }

    /// 行のファイルのパスと、検索で見つかった行の番号。
    fn row_path(&self, tab: usize, row: usize) -> Option<(PathBuf, u64)> {
        Some(match tab {
            TAB_SYNC => {
                let r = self.sync_rows.get(row)?;
                let root = self
                    .plan
                    .as_ref()
                    .map(|p| (p.src_root.clone(), p.dst_root.clone()))
                    .or_else(|| {
                        self.resume
                            .as_ref()
                            .map(|(_, r)| (r.src_root.clone(), r.dst_root.clone()))
                    })?;
                if r.item.src.is_some() {
                    (yy_files::join(&root.0, &r.item.rel), 0)
                } else {
                    (yy_files::join(&root.1, &r.item.dst_rel), 0)
                }
            }
            TAB_SEARCH => {
                let r = self.search_rows.get(row)?;
                let c = &self.search_cats[r.file.root];
                (c.path(&c.files[r.file.index]), r.line)
            }
            TAB_SIMILAR => {
                let &(g, m) = self.sim_rows.get(row)?;
                let f = self.sim_groups[g].members[m].file;
                let c = &self.sim_cats[f.root];
                (c.path(&c.files[f.index]), 0)
            }
            TAB_DUPES => {
                let &(g, k) = self.dup_rows.get(row)?;
                let f = self.dup_groups[g].files[k];
                let c = &self.dup_cats[f.root];
                (c.path(&c.files[f.index]), 0)
            }
            TAB_REVIEW => (self.review.items.get(row)?.path(), 0),
            _ => return None,
        })
    }

    /// 選んでいる行。
    fn selected_rows(&self, tab: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let Some(&l) = self.lists.get(tab) else {
            return out;
        };
        let mut i = -1isize;
        loop {
            i = unsafe {
                SendMessageW(
                    l,
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

    fn update_review_info(&self) {
        let (n, bytes) = self.review.checked_totals();
        set_text(
            self.edits.review_info,
            &format!(
                "{} 件中 {} 件（{}）をチェック",
                self.review.items.len(),
                n,
                yy_files::human_size(bytes)
            ),
        );
    }

    fn save_review(&mut self) {
        let p = self.dirs.reviews().join("current.review");
        if let Err(e) = self.review.save(&p) {
            self.log_line(&format!("削除の確認の一覧を保存できません: {e}"));
        }
    }

    /// 同期の設定（画面の欄から）。
    fn sync_options(&self) -> SyncOptions {
        let fm = &self.config.filemanager;
        let mode = unsafe { SendMessageW(self.edits.mode, CB_GETCURSEL, None, None).0 };
        let cmp = unsafe { SendMessageW(self.edits.compare, CB_GETCURSEL, None, None).0 };
        SyncOptions {
            mode: if mode == 1 {
                Mode::Mirror
            } else {
                Mode::Update
            },
            compare: if cmp == 1 {
                Compare::Content
            } else {
                Compare::SizeTime
            },
            time_tolerance: (fm.time_tolerance_sec * 1e9) as i64,
            threads: fm.copy_threads.max(1),
            verify_hash: fm.verify == "hash",
            checkpoint_bytes: 64 << 20,
            // 送り元が共有フォルダ（共有フォルダ → 手元）では、送り元のブロックを読むのに回線を使うので
            // 差分の送り方にしない
            delta_min: if is_network(Path::new(text_of(self.edits.src).trim())) {
                0
            } else {
                fm.delta_min_mb << 20
            },
            delta_block: 1 << 20,
            copy_acl: fm.copy_acl,
        }
    }

    fn scan_options(&self) -> yy_files::ScanOptions {
        work::scan_options(&self.config)
    }

    /// 保存した目録の使い方（`force` なら走査し直す）。
    fn catalog_cache(&self, force: bool) -> work::CatalogCache {
        work::CatalogCache {
            dir: self.dirs.catalogs(),
            max_age: self.config.filemanager.index_max_age_min as i64 * 60_000_000_000,
            now: now_ns(),
            force,
        }
    }

    /// 裏の処理（中身の索引の作成・中身の検索・索引のバキューム）の CPU・メモリの上限（設定）。
    fn background_limits(&self) -> yy_files::limits::Limits {
        let fm = &self.config.filemanager;
        yy_files::limits::Limits {
            cpu_percent: fm.background_cpu_percent.clamp(1, 100),
            memory: fm.background_memory_mb << 20,
        }
    }

    /// 中身を探す・中身の索引を作るファイルのパターン（設定。空なら既定の一覧）。
    fn content_patterns(&self) -> yy_files::pattern::Patterns {
        let p = &self.config.filemanager.content_patterns;
        if p.is_empty() {
            yy_files::pattern::Patterns::new(yy_files::pattern::DEFAULT_CONTENT_PATTERNS)
        } else {
            yy_files::pattern::Patterns::new(p)
        }
    }

    fn similar_options(&self) -> yy_files::similar::SimilarOptions {
        yy_files::similar::SimilarOptions {
            threshold: self.config.filemanager.similar_threshold,
            ..yy_files::similar::SimilarOptions::default()
        }
    }

    /// 処理を作業スレッドで始める（ほかの処理が動いていれば断る）。
    fn start(&mut self, what: &str, task: impl FnOnce(&work::Ctx) + Send + 'static) {
        if let Some((_, w)) = &self.busy {
            info_box(
                self.frame,
                &format!("「{w}」が終わってから行ってください（Esc で中止）。"),
            );
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.busy = Some((cancel.clone(), what.to_owned()));
        self.set_status(&format!("{what}…（Esc で中止）"));
        self.log_line(&format!("{what}を始めます"));
        work::spawn(Task {
            ctx: work::Ctx {
                notify: self.notify.clone(),
                cancel,
            },
            run: Box::new(task),
        });
    }
}

/// 隔離フォルダ（`.yyfm-trash`）を作る場所を人に選んでもらう（作業ごと）。`default` は「はい」で使う
/// 場所の説明。`Some(None)` は既定の場所、`Some(Some(p))` は選んだ場所、`None` はやめる。
fn choose_trash(frame: HWND, what: &str, default: &str) -> Option<Option<PathBuf>> {
    let text = format!(
        "{what}は、すぐには消さずに隔離フォルダ（{}）へ移します（元に戻せます）。\n\
         隔離フォルダをどこに作りますか？\n\n\
         はい: {default}に作る\n\
         いいえ: 場所を選ぶ（別のドライブ・共有なら、写してから元を消します）\n\
         キャンセル: やめる",
        yy_files::purge::TRASH_DIR
    );
    let r = unsafe {
        MessageBoxW(
            Some(frame),
            &HSTRING::from(text),
            w!("yyfilemanager - 隔離フォルダの場所"),
            MB_YESNOCANCEL | MB_ICONQUESTION,
        )
    };
    if r == IDYES {
        Some(None)
    } else if r == IDNO {
        crate::grepdlg::browse_folder(frame).map(Some)
    } else {
        None
    }
}

fn is_combo(h: HWND) -> bool {
    let mut buf = [0u16; 16];
    let n = unsafe { GetClassNameW(h, &mut buf) } as usize;
    String::from_utf16_lossy(&buf[..n]).eq_ignore_ascii_case("ComboBox")
}

// ---- 操作 ------------------------------------------------------------------------------

fn browse_into(edit: HWND, append: bool) {
    let Some(frame) = with(|a| a.frame) else {
        return;
    };
    if let Some(dir) = crate::grepdlg::browse_folder(frame) {
        let d = dir.to_string_lossy().into_owned();
        let cur = text_of(edit);
        if append && !cur.trim().is_empty() {
            set_text(edit, &format!("{}; {d}", cur.trim_end_matches([';', ' '])));
        } else {
            set_text(edit, &d);
        }
    }
}

fn load_job(name: &str) {
    with(|a| {
        let Some(j) = a.jobs.get(name).cloned() else {
            return;
        };
        set_text(a.edits.src, &j.src.to_string_lossy());
        set_text(a.edits.dst, &j.dst.to_string_lossy());
        unsafe {
            SendMessageW(
                a.edits.mode,
                CB_SETCURSEL,
                Some(WPARAM((j.options.mode == Mode::Mirror) as usize)),
                None,
            );
            SendMessageW(
                a.edits.compare,
                CB_SETCURSEL,
                Some(WPARAM((j.options.compare == Compare::Content) as usize)),
                None,
            );
        }
    });
}

fn save_job() {
    with(|a| {
        let name = text_of(a.edits.job).trim().to_owned();
        if name.is_empty() {
            info_box(a.frame, "同期ジョブの名前を入力してください。");
            return;
        }
        let options = a.sync_options();
        let dst = PathBuf::from(text_of(a.edits.dst).trim());
        // ミラーで消すファイルの隔離フォルダの場所は、ジョブを保存するときに選ぶ（定期実行で使う）
        let trash_root = if options.mode == Mode::Mirror {
            match choose_trash(
                a.frame,
                "このジョブのミラーで送り先から消すファイル",
                &format!("送り先（{}）", dst.display()),
            ) {
                Some(t) => t,
                None => return,
            }
        } else {
            None
        };
        let job = SyncJob {
            name: name.clone(),
            src: PathBuf::from(text_of(a.edits.src).trim()),
            dst,
            options,
            trash_root,
        };
        let new = a.jobs.get(&name).is_none();
        a.jobs.put(job);
        match a.jobs.save(&a.dirs.jobs_file()) {
            Ok(()) => {
                if new {
                    unsafe {
                        SendMessageW(
                            a.edits.job,
                            CB_ADDSTRING,
                            None,
                            Some(LPARAM(HSTRING::from(name.as_str()).as_ptr() as isize)),
                        );
                    }
                }
                a.set_status(&format!("同期ジョブ「{name}」を保存しました"));
            }
            Err(e) => error_box(a.frame, &format!("保存できません: {e}")),
        }
    });
}

fn delete_job() {
    with(|a| {
        let name = text_of(a.edits.job).trim().to_owned();
        if a.jobs.get(&name).is_none() {
            return;
        }
        a.jobs.remove(&name);
        let _ = a.jobs.save(&a.dirs.jobs_file());
        unsafe {
            SendMessageW(a.edits.job, CB_RESETCONTENT, None, None);
            for j in &a.jobs.jobs {
                SendMessageW(
                    a.edits.job,
                    CB_ADDSTRING,
                    None,
                    Some(LPARAM(HSTRING::from(j.name.as_str()).as_ptr() as isize)),
                );
            }
        }
        set_text(a.edits.job, "");
        a.set_status(&format!("同期ジョブ「{name}」を削除しました"));
    });
}

fn cmd_plan() {
    with(|a| {
        let src = PathBuf::from(text_of(a.edits.src).trim());
        let dst = PathBuf::from(text_of(a.edits.dst).trim());
        if src.as_os_str().is_empty() || dst.as_os_str().is_empty() {
            info_box(a.frame, "送り元と送り先を指定してください。");
            return;
        }
        let opts = a.sync_options();
        let scan = a.scan_options();
        let state_path = a.dirs.state_file(&dst);
        a.start("比べています", move |cx| {
            let state = yy_files::sync::SyncState::load(&state_path).ok();
            let r = yy_files::jobs::compare(
                &yy_files::Local,
                &src,
                &dst,
                &opts,
                &scan,
                state.as_ref(),
                &|p| cx.progress(&format!("比べています… {} 個のファイル", p.files)),
            );
            cx.send(Msg::Planned(r.map_err(|e| work::describe(&e))));
        });
    });
}

fn cmd_run(resume: bool) {
    with(|a| {
        let (run, journal) = if resume {
            match a.resume.clone() {
                Some((p, r)) => (r, p),
                None => {
                    info_box(a.frame, "続ける同期はありません。");
                    return;
                }
            }
        } else {
            let Some(plan) = &a.plan else {
                info_box(a.frame, "先に「比べる」で送るものを確かめてください。");
                return;
            };
            let (n, bytes) = plan.totals();
            if n == 0 {
                info_box(a.frame, "送るものはありません。");
                return;
            }
            let conflicts = plan.count(Action::Conflict);
            let text = format!(
                "{n} 件（{}）を送ります。{}\n\nよろしいですか？",
                yy_files::human_size(bytes),
                if conflicts > 0 {
                    format!("\n衝突の {conflicts} 件は送りません（右クリックで扱いを選べます）。")
                } else {
                    String::new()
                }
            );
            let r = unsafe {
                MessageBoxW(
                    Some(a.frame),
                    &HSTRING::from(text),
                    w!("yyfilemanager"),
                    MB_OKCANCEL | MB_ICONQUESTION,
                )
            };
            if r != IDOK {
                return;
            }
            let id = a.dirs.next_run_id();
            let opts = a.sync_options();
            let mut run = Run::new(id, plan, opts.mode, &local_stamp());
            let deletes = plan.count(Action::Delete);
            if deletes > 0 {
                match choose_trash(
                    a.frame,
                    &format!("ミラーで送り先から消す {deletes} 件"),
                    &format!("送り先（{}）", plan.dst_root.display()),
                ) {
                    Some(Some(t)) => run.trash_root = t,
                    Some(None) => {}
                    None => return,
                }
                a.log_line(&format!(
                    "隔離フォルダの場所: {}",
                    run.trash_root.join(yy_files::purge::TRASH_DIR).display()
                ));
            }
            (run, yy_files::sync::journal_path(&a.dirs.runs(), id))
        };
        // 一覧は実行の項目にする（何かするものだけ）
        a.sync_rows = run
            .items
            .iter()
            .map(|i| SyncRow {
                item: i.item.clone(),
                state: state_text(&i.state),
            })
            .collect();
        a.refresh_list(TAB_SYNC);
        let opts = a.sync_options();
        let state_path = a.dirs.state_file(&run.dst_root);
        a.start(
            if resume {
                "同期を続けています"
            } else {
                "同期しています"
            },
            move |cx| {
                let mut run = run;
                let mut state = yy_files::sync::SyncState::load(&state_path).unwrap_or_default();
                let total: u64 = run
                    .items
                    .iter()
                    .filter(|i| !matches!(i.state, ItemState::Done))
                    .filter_map(|i| i.item.src.map(|m| m.size))
                    .sum();
                let sent = std::sync::atomic::AtomicU64::new(0);
                let started = std::time::Instant::now();
                let r = yy_files::sync::execute(
                    &yy_files::Local,
                    &mut run,
                    &journal,
                    &opts,
                    &mut state,
                    &yy_files::sync::Hooks {
                        event: &|e| {
                            if let yy_files::sync::Event::Progress(_, n) = e {
                                let s = sent.fetch_add(n, Ordering::Relaxed) + n;
                                let secs = started.elapsed().as_secs_f64().max(0.001);
                                cx.progress(&format!(
                                    "同期しています… {} / {}（{}/秒）",
                                    yy_files::human_size(s),
                                    yy_files::human_size(total),
                                    yy_files::human_size((s as f64 / secs) as u64)
                                ));
                            } else {
                                cx.send(Msg::SyncEvent(e));
                            }
                        },
                        sleep: &|d| {
                            let until = std::time::Instant::now() + d;
                            while std::time::Instant::now() < until && !cx.cancelled() {
                                std::thread::sleep(std::time::Duration::from_millis(200));
                            }
                        },
                        cancel: &cx.cancel,
                    },
                );
                let _ = state.save(&state_path);
                let done = r.is_ok() && run.finished() && run.counts().failed == 0;
                if done {
                    let _ = std::fs::remove_file(&journal);
                }
                cx.send(Msg::SyncFinished(
                    run,
                    journal,
                    r.map_err(|e| work::describe(&e)),
                ));
            },
        );
    });
}

fn cmd_search() {
    with(|a| {
        let roots = roots_of(&text_of(a.edits.search_roots));
        if roots.is_empty() {
            info_box(a.frame, "探す場所（フォルダ）を指定してください。");
            return;
        }
        let q = match yy_files::search::parse_query(&text_of(a.edits.query), now_ns(), tz_offset())
        {
            Ok(q) => q,
            Err(e) => {
                error_box(a.frame, &e);
                return;
            }
        };
        let office = unsafe { SendMessageW(a.edits.office, BM_GETCHECK, None, None).0 } == 1;
        let force = unsafe { SendMessageW(a.edits.rescan, BM_GETCHECK, None, None).0 } == 1;
        let scan = a.scan_options();
        let cache = a.catalog_cache(force);
        let copts = yy_files::search::ContentOptions {
            office,
            pdf: office,
            patterns: a.content_patterns(),
            max_size: a.config.filemanager.search_max_mb << 20,
            // 裏の処理: 許した CPU の割合のスレッド数・メモリの上限・低い優先度
            threads: a
                .background_limits()
                .threads(a.config.filemanager.search_threads.max(1)),
            memory_limit: a.background_limits().memory,
            background: true,
            ..yy_files::search::ContentOptions::default()
        };
        a.search_rows.clear();
        a.search_cats.clear();
        a.refresh_list(TAB_SEARCH);
        let index_dir = a.dirs.index();
        let threads = a.config.filemanager.search_threads.max(1);
        let sim = a.similar_options();
        let fulltext_dir = a
            .config
            .filemanager
            .fulltext_index
            .then(|| a.dirs.fulltext());
        a.start("探しています", move |cx| {
            // 目録が古ければ、走査し直す前に古い目録で見つかったものを先に出す（名前・属性だけの検索）。
            // 前の目録がなければ、走査しながら見つかった順に出す
            let simple = q.content.is_none() && q.needs_marks() == (false, false);
            let mut previewed = false;
            if simple {
                let olds: Vec<(Catalog, i64)> = roots
                    .iter()
                    .filter_map(|r| yy_files::catalogs::load(&cache.dir, r).ok())
                    .filter(|st| roots.contains(&st.catalog.root))
                    .map(|st| (st.catalog, st.scanned_at))
                    .collect();
                let stale = olds
                    .iter()
                    .any(|(_, t)| cache.force || cache.now - t > cache.max_age);
                if olds.len() == roots.len() && stale {
                    let cats: Vec<Catalog> = olds.into_iter().map(|(c, _)| c).collect();
                    let rows = q
                        .filter(&cats)
                        .into_iter()
                        .map(|f| (f, 0, String::new()))
                        .collect();
                    cx.send(Msg::SearchPartial(cats, rows));
                    previewed = true;
                }
            }
            // 見つかった順に出す（0.2 秒ごとにまとめて送る）
            let pending: std::sync::Mutex<(Vec<(usize, yy_files::FileEntry)>, std::time::Instant)> =
                std::sync::Mutex::new((Vec::new(), std::time::Instant::now()));
            let flush = |force: bool| {
                let mut p = pending.lock().unwrap();
                if p.0.is_empty()
                    || (!force && p.1.elapsed() < std::time::Duration::from_millis(200))
                {
                    return;
                }
                p.1 = std::time::Instant::now();
                let mut by_root: std::collections::BTreeMap<usize, Vec<yy_files::FileEntry>> =
                    Default::default();
                for (ri, f) in p.0.drain(..) {
                    by_root.entry(ri).or_default().push(f);
                }
                for (ri, fs) in by_root {
                    cx.send(Msg::SearchFound(ri, roots[ri].clone(), fs));
                }
            };
            let stream = |ri: usize, fs: &[yy_files::FileEntry]| {
                let hit: Vec<(usize, yy_files::FileEntry)> = fs
                    .iter()
                    .filter(|f| q.matches(f))
                    .map(|f| (ri, f.clone()))
                    .collect();
                if !hit.is_empty() {
                    pending.lock().unwrap().0.extend(hit);
                }
                flush(false);
            };
            let found: Option<&work::FoundIn<'_>> = (simple && !previewed).then_some(&stream as _);
            let cats = match work::scan_all(cx, &roots, &scan, Some(&cache), found) {
                Ok(c) => c,
                Err(e) => {
                    cx.send(Msg::Searched(Err(e)));
                    return;
                }
            };
            flush(true);
            let mut found = q.filter(&cats);
            // 整理の結果の条件（重複・版の判定）
            let (need_dupes, need_versions) = q.needs_marks();
            if need_dupes || need_versions {
                let mut marks = yy_files::search::Marks::default();
                if need_versions {
                    cx.progress("版を判定しています…");
                    marks.set_versions(&yy_files::similar::find_versions(&cats, &sim));
                }
                if need_dupes {
                    match work::find_dupes(cx, &cats, &index_dir, threads, true) {
                        Ok(g) => marks.set_dupes(&g),
                        Err(e) => {
                            cx.send(Msg::Searched(Err(work::describe(&e))));
                            return;
                        }
                    }
                }
                found = q.apply_marks(found, &marks);
            }
            let Some(content) = q.content.clone() else {
                let rows = found.into_iter().map(|f| (f, 0, String::new())).collect();
                cx.send(Msg::Searched(Ok((cats, rows))));
                return;
            };
            let hits = std::sync::Mutex::new(Vec::new());
            let total = found.len();
            let progress = |n| cx.progress(&format!("中身を探しています… {n} / {total} 個"));
            let rec = |h: yy_files::search::Hit| {
                hits.lock().unwrap().push((h.file, h.line, h.text));
                true
            };
            let r = match &fulltext_dir {
                // 中身の索引で読むファイルを絞り、読んだものは索引に足す
                Some(dir) => {
                    // 索引は場所ごとに 1 つずつ読み込み、探し終えたら保存して手放す（メモリを抑える）
                    let path_of = |ri: usize| yy_files::fulltext::path_for(dir, &cats[ri].root);
                    let load = |ri: usize| {
                        yy_files::fulltext::FtIndex::load(&path_of(ri))
                            .ok()
                            .filter(|ix| ix.root == cats[ri].root)
                            .unwrap_or_else(|| yy_files::fulltext::FtIndex::new(&cats[ri].root))
                    };
                    let save = |ri: usize, ix: &mut yy_files::fulltext::FtIndex| {
                        ix.retain_catalog(&cats[ri]);
                        if let Err(e) = ix.save(&path_of(ri)) {
                            cx.send(Msg::Log(format!("中身の索引を保存できません: {e}")));
                        }
                    };
                    let r = yy_files::search::search_content_indexed_by_root(
                        &cats, &found, &content, &copts, &load, &save, &progress, &rec,
                    );
                    if let Ok(st) = &r {
                        cx.send(Msg::Log(format!(
                            "中身の索引: {} 個を読まずに済みました・{} 個を索引に足しました（パターンに合わず読まなかったもの {} 個）",
                            st.pruned, st.indexed, st.excluded
                        )));
                        if st.not_indexed > 0 {
                            cx.send(Msg::Log(format!(
                                "中身の索引: メモリの上限（background_memory_mb）のため {} 個は索引に足さずに探しました",
                                st.not_indexed
                            )));
                        }
                    }
                    r
                }
                None => yy_files::search::search_content(
                    &cats, &found, &content, &copts, &progress, &rec,
                ),
            };
            let mut rows = hits.into_inner().unwrap();
            rows.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
            match r {
                Ok(_) => cx.send(Msg::Searched(Ok((cats, rows)))),
                Err(e) if yy_files::is_cancelled(&e) => cx.send(Msg::Searched(Ok((cats, rows)))),
                Err(e) => cx.send(Msg::Searched(Err(work::describe(&e)))),
            }
        });
    });
}

fn run_saved_search(name: &str) {
    let found = with(|a| {
        let Some(s) = a.searches.get(name).cloned() else {
            return false;
        };
        set_text(a.edits.query, &s.query);
        let roots: Vec<String> = s
            .roots
            .iter()
            .map(|r| r.to_string_lossy().into_owned())
            .collect();
        set_text(a.edits.search_roots, &roots.join("; "));
        unsafe {
            SendMessageW(
                a.edits.office,
                BM_SETCHECK,
                Some(WPARAM(s.office as usize)),
                None,
            );
        }
        true
    });
    if found == Some(true) {
        cmd_search();
    }
}

fn refill_saved(a: &App) {
    unsafe {
        SendMessageW(a.edits.saved, CB_RESETCONTENT, None, None);
        for s in &a.searches.searches {
            SendMessageW(
                a.edits.saved,
                CB_ADDSTRING,
                None,
                Some(LPARAM(HSTRING::from(s.name.as_str()).as_ptr() as isize)),
            );
        }
    }
}

fn save_search() {
    with(|a| {
        let name = text_of(a.edits.saved).trim().to_owned();
        if name.is_empty() {
            info_box(
                a.frame,
                "「保存した検索」の欄に名前を入力してから保存してください。",
            );
            return;
        }
        let query = text_of(a.edits.query).trim().to_owned();
        if let Err(e) = yy_files::search::parse_query(&query, now_ns(), tz_offset()) {
            error_box(a.frame, &e);
            return;
        }
        a.searches.put(SavedSearch {
            name: name.clone(),
            query,
            roots: roots_of(&text_of(a.edits.search_roots)),
            office: unsafe { SendMessageW(a.edits.office, BM_GETCHECK, None, None).0 } == 1,
        });
        match a.searches.save(&a.dirs.searches_file()) {
            Ok(()) => {
                refill_saved(a);
                set_text(a.edits.saved, &name);
                a.set_status(&format!("検索「{name}」を保存しました"));
            }
            Err(e) => error_box(a.frame, &format!("保存できません: {e}")),
        }
    });
}

fn delete_search() {
    with(|a| {
        let name = text_of(a.edits.saved).trim().to_owned();
        if a.searches.get(&name).is_none() {
            return;
        }
        a.searches.remove(&name);
        let _ = a.searches.save(&a.dirs.searches_file());
        refill_saved(a);
        set_text(a.edits.saved, "");
        a.set_status(&format!("検索「{name}」を削除しました"));
    });
}

/// 検索の結果の 1 件から、別の版（`versions`）か同じ中身のファイルを、検索した場所の中で探す
/// （18 章 8.4）。結果は「似たファイル」「重複」のタブに出す。
fn find_related(row: usize, versions: bool) {
    with(|a| {
        let Some(file) = a.search_rows.get(row).map(|r| r.file) else {
            return;
        };
        let cats = a.search_cats.clone();
        let name = cats[file.root].files[file.index].name().to_owned();
        if versions {
            let opts = a.similar_options();
            a.start(
                &format!("「{name}」の別の版を探しています"),
                move |cx| {
                    let groups: Vec<VersionGroup> =
                        yy_files::similar::versions_of(&cats, file, &opts)
                            .into_iter()
                            .collect();
                    if groups.is_empty() {
                        cx.send(Msg::Log(format!(
                            "「{name}」の別の版は見つかりませんでした"
                        )));
                    }
                    cx.send(Msg::Similar(Ok((cats, groups))));
                },
            );
        } else {
            a.start(
                &format!("「{name}」と同じ中身のファイルを探しています"),
                move |cx| {
                    let r = yy_files::dupes::same_content(&yy_files::Local, &cats, file, &|| {
                        cx.cancelled()
                    });
                    match r {
                        Ok(g) => {
                            if g.is_none() {
                                cx.send(Msg::Log(format!(
                                    "「{name}」と同じ中身のファイルは見つかりませんでした"
                                )));
                            }
                            cx.send(Msg::Dupes(Ok((cats, g.into_iter().collect()))));
                        }
                        Err(e) => cx.send(Msg::Dupes(Err(work::describe(&e)))),
                    }
                },
            );
        }
    });
}

/// 中身の索引のバキューム: 対象のパターンに合わなくなった・消えた・変わったファイルの記録を除いて
/// 詰め直す。場所がなくなった索引は消す。
fn vacuum_fulltext() {
    with(|a| {
        let dir = a.dirs.fulltext();
        let keep = a.content_patterns();
        let text = format!(
            "中身の索引（{}）をバキュームします。\n\n\
             ・中身を探すパターンに合わなくなったファイル、消えたファイル、変わったファイルの記録を除きます。\n\
             ・場所（フォルダ）がなくなった索引は消します。\n\
             ・ファイルがあるかを確かめるので、共有フォルダでは時間がかかることがあります。\n\n\
             よろしいですか？",
            dir.display()
        );
        let ok = unsafe {
            MessageBoxW(
                Some(a.frame),
                &HSTRING::from(text),
                w!("yyfilemanager"),
                MB_OKCANCEL | MB_ICONQUESTION,
            )
        } == IDOK;
        if !ok {
            return;
        }
        a.start("中身の索引をバキュームしています", move |cx| {
            // 裏の処理として優先度を下げる（索引は 1 つずつ読み込む）
            yy_files::limits::enter_background();
            let r = yy_files::fulltext::vacuum_dir(&yy_files::Local, &dir, &keep, &|p| {
                cx.progress(&format!("{} をバキュームしています…", p.display()))
            });
            match r {
                Ok(reps) => {
                    let (mut before, mut after, mut removed) = (0u64, 0u64, 0usize);
                    for rep in &reps {
                        before += rep.bytes_before;
                        after += rep.bytes_after;
                        removed += rep.stats.dead;
                        cx.send(Msg::Log(if rep.deleted {
                            format!(
                                "中身の索引を消しました（場所がない・読めない）: {} {}",
                                rep.root.display(),
                                rep.file.display()
                            )
                        } else {
                            format!(
                                "中身の索引: {}: 対象外 {}・消えた {}・変わった {} を除き、{} 件を残しました（{} → {}）",
                                rep.root.display(),
                                rep.stats.unnamed,
                                rep.stats.gone,
                                rep.stats.stale,
                                rep.stats.kept,
                                yy_files::human_size(rep.bytes_before),
                                yy_files::human_size(rep.bytes_after)
                            )
                        }));
                    }
                    cx.send(Msg::Done(format!(
                        "中身の索引をバキュームしました: {} 個の索引から {removed} 件の記録を除きました（{} → {}）",
                        reps.len(),
                        yy_files::human_size(before),
                        yy_files::human_size(after)
                    )));
                }
                Err(e) => cx.send(Msg::Done(format!(
                    "中身の索引のバキュームを中断しました: {}",
                    work::describe(&e)
                ))),
            }
        });
    });
}

/// 隔離フォルダ（`.yyfm-trash`）のうち、設定の日数（`trash_days`）より古いものを消す。
fn expire_trash() {
    let Some((frame, days)) = with(|a| (a.frame, a.config.filemanager.trash_days)) else {
        return;
    };
    let Some(root) = crate::grepdlg::browse_folder(frame) else {
        return;
    };
    let limit = yy_files::sync::stamp(
        now_ns() + tz_offset() * 1_000_000_000 - days as i64 * 86_400_000_000_000,
    )
    .replace([' ', ':'], "-");
    let old = match yy_files::purge::expired_trash(&yy_files::Local, &root, &limit) {
        Ok(v) => v,
        Err(e) => {
            error_box(frame, &format!("隔離フォルダを読めません: {e}"));
            return;
        }
    };
    if old.is_empty() {
        info_box(
            frame,
            &format!(
                "{} の隔離フォルダに、{days} 日より古いものはありません。",
                root.display()
            ),
        );
        return;
    }
    let names: Vec<String> = old
        .iter()
        .take(10)
        .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect();
    let text = format!(
        "{} の隔離フォルダの、{days} 日より古いもの {} 個を消します（元に戻せません）。\n\n{}{}\n\nよろしいですか？",
        root.display(),
        old.len(),
        names.join("\n"),
        if old.len() > names.len() { "\n…" } else { "" }
    );
    let ok = unsafe {
        MessageBoxW(
            Some(frame),
            &HSTRING::from(text),
            w!("yyfilemanager"),
            MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2,
        )
    } == IDYES;
    if !ok {
        return;
    }
    with(|a| {
        a.start(
            "隔離フォルダの古いものを消しています",
            move |cx| {
                let mut removed = 0;
                for d in &old {
                    if cx.cancelled() {
                        break;
                    }
                    cx.progress(&format!("{} を消しています…", d.display()));
                    match yy_files::purge::remove_tree(&yy_files::Local, d) {
                        Ok(()) => {
                            removed += 1;
                            cx.send(Msg::Log(format!(
                                "隔離フォルダを消しました: {}",
                                d.display()
                            )));
                        }
                        Err(e) => cx.send(Msg::Log(format!(
                            "隔離フォルダを消せません: {}: {}",
                            d.display(),
                            work::describe(&e)
                        ))),
                    }
                }
                cx.send(Msg::Done(format!(
                    "隔離フォルダの古いもの {removed} / {} 個を消しました",
                    old.len()
                )));
            },
        );
    });
}

fn cmd_similar() {
    with(|a| {
        let roots = roots_of(&text_of(a.edits.sim_roots));
        if roots.is_empty() {
            info_box(a.frame, "探す場所（フォルダ）を指定してください。");
            return;
        }
        let scan = a.scan_options();
        let opts = a.similar_options();
        let cache = a.catalog_cache(false);
        // 対象の拡張子・名前の正規表現（空なら全部）
        let filter = match yy_files::search::FileFilter::parse(
            &text_of(a.edits.sim_exts),
            &text_of(a.edits.sim_regex),
        ) {
            Ok(f) => f,
            Err(e) => {
                error_box(a.frame, &e);
                return;
            }
        };
        a.start("似たファイルを探しています", move |cx| {
            let mut cats = match work::scan_all(cx, &roots, &scan, Some(&cache), None) {
                Ok(c) => c,
                Err(e) => {
                    cx.send(Msg::Similar(Err(e)));
                    return;
                }
            };
            filter.apply(&mut cats);
            cx.progress("名前を比べています…");
            let groups = yy_files::similar::find_versions(&cats, &opts);
            cx.send(Msg::Similar(Ok((cats, groups))));
        });
    });
}

fn cmd_dupes() {
    with(|a| {
        let roots = roots_of(&text_of(a.edits.dup_roots));
        if roots.is_empty() {
            info_box(a.frame, "探す場所（フォルダ）を指定してください。");
            return;
        }
        let scan = a.scan_options();
        let index_dir = a.dirs.index();
        let threads = a.config.filemanager.search_threads.max(1);
        // 中身を読むので目録は使い回さない（走査し直して保存する）
        let cache = a.catalog_cache(true);
        let filter = match yy_files::search::FileFilter::parse(
            &text_of(a.edits.dup_exts),
            &text_of(a.edits.dup_regex),
        ) {
            Ok(f) => f,
            Err(e) => {
                error_box(a.frame, &e);
                return;
            }
        };
        a.start("重複を探しています", move |cx| {
            let mut cats = match work::scan_all(cx, &roots, &scan, Some(&cache), None) {
                Ok(c) => c,
                Err(e) => {
                    cx.send(Msg::Dupes(Err(e)));
                    return;
                }
            };
            // 絞り込んだときは、対象外のファイルのハッシュの覚え書きを索引から消さない
            let prune = filter.is_empty();
            filter.apply(&mut cats);
            let r = work::find_dupes(cx, &cats, &index_dir, threads, prune);
            cx.send(Msg::Dupes(
                r.map(|g| (cats, g)).map_err(|e| work::describe(&e)),
            ));
        });
    });
}

/// 似たファイルの古い版・重複の写しを削除の確認に加える（`rows` は一覧の行。空なら全部）。
fn to_review(tab: usize, rows: Vec<usize>) {
    with(|a| {
        let before = a.review.items.len();
        let next_group = a
            .review
            .items
            .iter()
            .map(|c| c.group + 1)
            .max()
            .unwrap_or(0);
        let pick: Vec<usize> = if rows.is_empty() {
            (0..a.row_count(tab)).collect()
        } else {
            rows
        };
        if tab == TAB_SEARCH {
            // 検索の結果は 1 件ずつ（残す相手がないので、チェックは人が付ける）
            let mut added = 0;
            let mut seen = std::collections::HashSet::new();
            for (k, r) in pick.iter().enumerate() {
                let Some(row) = a.search_rows.get(*r) else {
                    continue;
                };
                let c = &a.search_cats[row.file.root];
                let f = &c.files[row.file.index];
                let p = c.path(f);
                if !seen.insert(p.clone()) || a.review.items.iter().any(|x| x.path() == p) {
                    continue;
                }
                a.review.items.push(Candidate {
                    root: c.root.clone(),
                    rel: f.rel.clone(),
                    meta: f.meta,
                    group: next_group + k,
                    keep: false,
                    checked: false,
                    reason: "検索から選んだ".into(),
                    confidence: String::new(),
                    dupe_of: None,
                    alone: true,
                });
                added += 1;
            }
            a.save_review();
            a.refresh_list(TAB_REVIEW);
            a.update_review_info();
            a.set_status(&format!(
                "削除の確認に {added} 件を加えました（チェックを付けたものだけを消します）"
            ));
            return;
        }
        let mut groups: Vec<usize> = Vec::new();
        for r in &pick {
            let g = match tab {
                TAB_SIMILAR => a.sim_rows.get(*r).map(|x| x.0),
                TAB_DUPES => a.dup_rows.get(*r).map(|x| x.0),
                _ => None,
            };
            if let Some(g) = g
                && !groups.contains(&g)
            {
                groups.push(g);
            }
        }
        let exists = |a: &App, p: &Path| a.review.items.iter().any(|c| c.path() == p);
        for (k, g) in groups.iter().enumerate() {
            let gid = next_group + k;
            match tab {
                TAB_SIMILAR => {
                    let grp = a.sim_groups[*g].clone();
                    for (m, mem) in grp.members.iter().enumerate() {
                        let c = &a.sim_cats[mem.file.root];
                        let f = &c.files[mem.file.index];
                        if exists(a, &c.path(f)) {
                            continue;
                        }
                        a.review.items.push(Candidate {
                            root: c.root.clone(),
                            rel: f.rel.clone(),
                            meta: f.meta,
                            group: gid,
                            keep: m == 0,
                            checked: m != 0
                                && grp.confidence == yy_files::similar::Confidence::High,
                            reason: if m == 0 {
                                "最新（提案）".into()
                            } else {
                                "古い版".into()
                            },
                            confidence: grp.confidence.label().into(),
                            dupe_of: None,
                            alone: false,
                        });
                    }
                }
                TAB_DUPES => {
                    let grp = a.dup_groups[*g].clone();
                    let keeper = {
                        let r = grp.files[0];
                        let c = &a.dup_cats[r.root];
                        c.path(&c.files[r.index])
                    };
                    for (k2, r) in grp.files.iter().enumerate() {
                        let c = &a.dup_cats[r.root];
                        let f = &c.files[r.index];
                        if exists(a, &c.path(f)) {
                            continue;
                        }
                        a.review.items.push(Candidate {
                            root: c.root.clone(),
                            rel: f.rel.clone(),
                            meta: f.meta,
                            group: gid,
                            keep: k2 == 0,
                            checked: k2 != 0,
                            reason: if k2 == 0 {
                                "残す（重複）".into()
                            } else {
                                "重複の写し".into()
                            },
                            confidence: "高".into(),
                            dupe_of: (k2 != 0).then(|| (grp.hash, keeper.clone())),
                            alone: false,
                        });
                    }
                }
                _ => {}
            }
        }
        let added = a.review.items.len() - before;
        a.save_review();
        a.refresh_list(TAB_REVIEW);
        a.update_review_info();
        a.set_status(&format!("削除の確認に {added} 件を加えました"));
    });
}

fn cmd_purge() {
    with(|a| {
        let (n, bytes) = a.review.checked_totals();
        if n == 0 {
            info_box(a.frame, "チェックしたファイルがありません。");
            return;
        }
        let emptied = a.review.emptied_groups().len();
        // 共有フォルダのファイルは隔離フォルダへ移す。その場所を選んでもらう
        let shared = a
            .review
            .items
            .iter()
            .filter(|c| c.checked && !c.keep && is_network(&c.root))
            .count();
        let trash_base = if shared > 0 {
            match choose_trash(
                a.frame,
                &format!("共有フォルダのファイル {shared} 件"),
                "それぞれのファイルのある共有（同期・検索したフォルダ）",
            ) {
                Some(t) => t,
                None => return,
            }
        } else {
            None
        };
        let where_ = match &trash_base {
            Some(b) => b.join(yy_files::purge::TRASH_DIR).display().to_string(),
            None => format!("それぞれの共有の {}", yy_files::purge::TRASH_DIR),
        };
        let text = format!(
            "チェックした {n} 件（{}）を削除します。\n\n\
             ・手元のドライブのファイルはごみ箱へ送ります。\n\
             ・共有フォルダのファイルは、隔離フォルダ（{where_}）へ移します（{} 日後に消せます）。\n{}\n\
             よろしいですか？",
            yy_files::human_size(bytes),
            a.config.filemanager.trash_days,
            if emptied > 0 {
                format!(
                    "・全部にチェックが付いた {emptied} グループは、最後の 1 つを残すため消しません。\n"
                )
            } else {
                String::new()
            }
        );
        let r = unsafe {
            MessageBoxW(
                Some(a.frame),
                &HSTRING::from(text),
                w!("yyfilemanager"),
                MB_OKCANCEL | MB_ICONWARNING | MB_DEFBUTTON2,
            )
        };
        if r != IDOK {
            return;
        }
        start_purge(a, None, trash_base);
    });
}

/// 削除を始める（`only` を渡したら、そのファイルだけを隔離フォルダを使わずにすぐに消す）。
fn start_purge(
    a: &mut App,
    only: Option<std::collections::HashSet<PathBuf>>,
    trash_base: Option<PathBuf>,
) {
    let direct = only.is_some();
    let mut review = a.review.clone();
    if let Some(only) = &only {
        for c in &mut review.items {
            c.checked = c.checked && only.contains(&c.path());
        }
    }
    let protect: Vec<PathBuf> = a
        .config
        .filemanager
        .protect
        .iter()
        .map(PathBuf::from)
        .collect();
    let log = a
        .dirs
        .purges()
        .join(format!("{}.csv", local_stamp().replace([' ', ':'], "-")));
    let stamp = local_stamp().replace([' ', ':'], "-");
    a.start("削除しています", move |cx| {
        let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        let mut done = 0usize;
        let r = yy_files::purge::execute(
            &yy_files::Local,
            &review,
            &stamp,
            trash_base.as_deref(),
            &protect,
            &log,
            yy_files::purge::PurgeHooks {
                dispose: &|c| {
                    if direct {
                        yy_files::purge::Disposal::Delete
                    } else if is_network(&c.root) {
                        yy_files::purge::Disposal::Trash
                    } else {
                        yy_files::purge::Disposal::RecycleBin
                    }
                },
                recycle: &recycle,
                progress: &mut |_, _| {
                    done += 1;
                    cx.progress(&format!("削除しています… {done} 件"));
                    !cx.cancelled()
                },
            },
        );
        cx.send(Msg::Purged(r.map_err(|e| work::describe(&e)), log));
    });
}

fn cmd_undo() {
    let Some((frame, dir)) = with(|a| (a.frame, a.dirs.purges())) else {
        return;
    };
    let Some(log) = pick_file(frame, ("削除の記録 (*.csv)", "*.csv"), Some(&dir), None) else {
        return;
    };
    match yy_files::purge::undo(&yy_files::Local, &log) {
        Ok(rs) => {
            use yy_files::purge::Restore;
            let ok = rs
                .iter()
                .filter(|r| matches!(r, Restore::Restored(_)))
                .count();
            let exists = rs
                .iter()
                .filter(|r| matches!(r, Restore::Exists(_)))
                .count();
            let other = rs.len() - ok - exists;
            let msg = format!(
                "{ok} 件を元の場所に戻しました。{}{}",
                if exists > 0 {
                    format!("\n元の場所に同じ名前がある {exists} 件は戻していません。")
                } else {
                    String::new()
                },
                if other > 0 {
                    format!(
                        "\nごみ箱へ送った・すぐに消した・戻せなかった {other} 件があります（記録のタブを見てください）。"
                    )
                } else {
                    String::new()
                }
            );
            with(|a| {
                for r in &rs {
                    a.log_line(&format!("元に戻す: {r:?}"));
                }
            });
            info_box(frame, &msg);
        }
        Err(e) => error_box(frame, &format!("{}\n\n{e}", log.display())),
    }
}

/// 作業スレッドからの知らせを受ける。
fn on_messages() {
    loop {
        let Some(Some(m)) = with(|a| a.notify.take_pending(&a.rx)) else {
            return;
        };
        handle(m);
    }
}

fn finish(a: &mut App, msg: &str) {
    a.busy = None;
    a.set_status(msg);
    a.log_line(msg);
}

fn handle(m: Msg) {
    match m {
        Msg::Panicked(s) => {
            with(|a| {
                finish(a, &format!("処理が異常終了しました: {s}"));
            });
        }
        Msg::Progress(s) => {
            with(|a| a.set_status(&format!("{s}（Esc で中止）")));
        }
        Msg::Log(s) => {
            with(|a| a.log_line(&s));
        }
        Msg::Done(s) => {
            with(|a| finish(a, &s));
        }
        Msg::Planned(r) => {
            with(|a| match r {
                Ok(p) => {
                    let (n, bytes) = p.totals();
                    let msg = format!(
                        "比べました: 新規 {}・更新 {}・日時だけ {}・削除 {}・衝突 {}（{} 件・{} を送ります）",
                        p.count(Action::New),
                        p.count(Action::Update),
                        p.count(Action::Touch),
                        p.count(Action::Delete),
                        p.count(Action::Conflict),
                        n,
                        yy_files::human_size(bytes)
                    );
                    a.sync_rows = p
                        .items
                        .iter()
                        .filter(|i| i.action != Action::Same)
                        .map(|i| SyncRow {
                            item: i.clone(),
                            state: String::new(),
                        })
                        .collect();
                    set_text(a.edits.sync_info, &msg);
                    a.plan = Some(p);
                    a.refresh_list(TAB_SYNC);
                    finish(a, &msg);
                }
                Err(e) => {
                    finish(a, "比べられませんでした");
                    error_box(a.frame, &e);
                }
            });
        }
        Msg::SyncEvent(e) => {
            with(|a| {
                use yy_files::sync::Event;
                let set = |a: &mut App, i: usize, s: String| {
                    if let Some(r) = a.sync_rows.get_mut(i) {
                        r.state = s;
                    }
                    unsafe {
                        SendMessageW(
                            a.lists[TAB_SYNC],
                            LVM_REDRAWITEMS,
                            Some(WPARAM(i)),
                            Some(LPARAM(i as isize)),
                        );
                    }
                };
                match e {
                    Event::Started(i) => set(a, i, "送っています".into()),
                    Event::Done(i) => set(a, i, "済み".into()),
                    Event::Failed(i, m) => {
                        let rel = a
                            .sync_rows
                            .get(i)
                            .map(|r| r.item.rel.clone())
                            .unwrap_or_default();
                        a.log_line(&format!("{rel}: 失敗: {m}"));
                        set(a, i, format!("失敗: {m}"));
                    }
                    Event::Retry(i, secs, n) => {
                        set(a, i, format!("{secs} 秒後に続けます（{n} 回目）"))
                    }
                    Event::Log(s) => a.log_line(&s),
                    Event::Progress(..) => {}
                }
            });
        }
        Msg::SyncFinished(run, journal, r) => {
            with(|a| {
                let c = run.counts();
                a.sync_rows = run
                    .items
                    .iter()
                    .map(|i| SyncRow {
                        item: i.item.clone(),
                        state: state_text(&i.state),
                    })
                    .collect();
                a.refresh_list(TAB_SYNC);
                a.plan = None;
                let msg = match &r {
                    Ok(_) => format!("同期しました: 済み {}・失敗 {}", c.done, c.failed),
                    Err(e) => format!(
                        "同期を中断しました（済み {}・残り {}）: {e}。「続ける」で続きから送ります",
                        c.done, c.pending
                    ),
                };
                a.resume = (!run.finished() || c.failed > 0)
                    .then_some((journal, run))
                    .filter(|(_, r)| !r.finished());
                set_text(a.edits.sync_info, &msg);
                finish(a, &msg);
            });
        }
        Msg::Searched(r) => {
            with(|a| match r {
                Ok((cats, rows)) => {
                    a.search_cats = cats;
                    a.search_rows = rows
                        .into_iter()
                        .map(|(file, line, text)| SearchRow { file, line, text })
                        .collect();
                    a.refresh_list(TAB_SEARCH);
                    let msg = format!("{} 件見つかりました", a.search_rows.len());
                    finish(a, &msg);
                }
                Err(e) => {
                    finish(a, "探せませんでした");
                    error_box(a.frame, &e);
                }
            });
        }
        Msg::SearchPartial(cats, rows) => {
            with(|a| {
                a.search_cats = cats;
                a.search_rows = rows
                    .into_iter()
                    .map(|(file, line, text)| SearchRow { file, line, text })
                    .collect();
                a.refresh_list(TAB_SEARCH);
                let msg = format!(
                    "前回の目録で {} 件見つかりました。走査し直しています…（Esc で中止）",
                    a.search_rows.len()
                );
                a.set_status(&msg);
            });
        }
        Msg::SearchFound(ri, root, files) => {
            with(|a| {
                // 走査しながらの結果は、仮の目録（場所ごと）に足していく。走査し終えたら置き換わる
                while a.search_cats.len() <= ri {
                    a.search_cats.push(Catalog::default());
                }
                let c = &mut a.search_cats[ri];
                if c.root != root {
                    *c = Catalog {
                        root,
                        ..Catalog::default()
                    };
                }
                for f in files {
                    a.search_rows.push(SearchRow {
                        file: FileRef {
                            root: ri,
                            index: c.files.len(),
                        },
                        line: 0,
                        text: String::new(),
                    });
                    c.files.push(f);
                }
                a.refresh_list(TAB_SEARCH);
                let msg = format!(
                    "走査しながら {} 件見つかりました…（Esc で中止）",
                    a.search_rows.len()
                );
                a.set_status(&msg);
            });
        }
        Msg::Similar(r) => {
            with(|a| match r {
                Ok((cats, groups)) => {
                    a.sim_rows = groups
                        .iter()
                        .enumerate()
                        .flat_map(|(g, grp)| (0..grp.members.len()).map(move |m| (g, m)))
                        .collect();
                    let high = groups
                        .iter()
                        .filter(|g| g.confidence == yy_files::similar::Confidence::High)
                        .count();
                    a.sim_cats = cats;
                    a.sim_groups = groups;
                    a.refresh_list(TAB_SIMILAR);
                    if a.current != TAB_SIMILAR {
                        a.show_tab(TAB_SIMILAR);
                    }
                    let msg = format!(
                        "似たファイルのグループが {} 個（判定の自信が高いもの {} 個）",
                        a.sim_groups.len(),
                        high
                    );
                    finish(a, &msg);
                }
                Err(e) => {
                    finish(a, "探せませんでした");
                    error_box(a.frame, &e);
                }
            });
        }
        Msg::Dupes(r) => {
            with(|a| match r {
                Ok((cats, groups)) => {
                    a.dup_rows = groups
                        .iter()
                        .enumerate()
                        .flat_map(|(g, grp)| (0..grp.files.len()).map(move |k| (g, k)))
                        .collect();
                    let wasted: u64 = groups.iter().map(Group::wasted).sum();
                    a.dup_cats = cats;
                    a.dup_groups = groups;
                    a.refresh_list(TAB_DUPES);
                    if a.current != TAB_DUPES {
                        a.show_tab(TAB_DUPES);
                    }
                    let msg = format!(
                        "重複のグループが {} 個（写しを消すと {} 空きます）",
                        a.dup_groups.len(),
                        yy_files::human_size(wasted)
                    );
                    finish(a, &msg);
                }
                Err(e) => {
                    finish(a, "探せませんでした");
                    error_box(a.frame, &e);
                }
            });
        }
        Msg::Purged(r, log) => {
            let Some(frame) = with(|a| a.frame) else {
                return;
            };
            let report = match r {
                Ok(rep) => rep,
                Err(e) => {
                    with(|a| finish(a, "削除できませんでした"));
                    error_box(frame, &e);
                    return;
                }
            };
            use yy_files::purge::Outcome;
            let no_trash: std::collections::HashSet<PathBuf> = with(|a| {
                report
                    .outcomes
                    .iter()
                    .filter(|(_, o)| matches!(o, Outcome::NoTrash(_)))
                    .filter_map(|(i, _)| a.review.items.get(*i).map(Candidate::path))
                    .collect()
            })
            .unwrap_or_default();
            with(|a| {
                for (i, o) in &report.outcomes {
                    if let Some(c) = a.review.items.get(*i) {
                        let p = c.path();
                        a.log_line(&format!("削除: {}: {o:?}", p.display()));
                    }
                }
                // 消したものを一覧から外す
                let removed: std::collections::HashSet<usize> = report
                    .outcomes
                    .iter()
                    .filter(|(_, o)| matches!(o, Outcome::Removed(..)))
                    .map(|(i, _)| *i)
                    .collect();
                let mut k = 0;
                a.review.items.retain(|_| {
                    let keep = !removed.contains(&k);
                    k += 1;
                    keep
                });
                // グループに 1 つしか残らなければ、そのグループは一覧から外す
                let mut counts: std::collections::HashMap<usize, usize> = Default::default();
                for c in &a.review.items {
                    *counts.entry(c.group).or_default() += 1;
                }
                a.review.items.retain(|c| counts[&c.group] > 1);
                a.save_review();
                a.refresh_list(TAB_REVIEW);
                a.update_review_info();
                let skipped = report.outcomes.len() - report.removed - no_trash.len();
                let msg = format!(
                    "{} 件（{}）を削除しました。{}（記録: {}）",
                    report.removed,
                    yy_files::human_size(report.bytes),
                    if skipped > 0 {
                        format!("消さなかったもの {skipped} 件")
                    } else {
                        String::new()
                    },
                    log.display()
                );
                finish(a, &msg);
            });
            if !no_trash.is_empty() {
                // 隔離フォルダを作れない共有: 確認を 2 回したうえで、すぐに消す
                let text = format!(
                    "{} 件は、共有フォルダに隔離フォルダを作れないため移せませんでした。\n\nすぐに（元に戻せない形で）消しますか？",
                    no_trash.len()
                );
                let ok = |t: &str| unsafe {
                    MessageBoxW(
                        Some(frame),
                        &HSTRING::from(t),
                        w!("yyfilemanager"),
                        MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2,
                    ) == IDYES
                };
                if ok(&text) && ok("本当に消しますか？ 消したファイルは元に戻せません。")
                {
                    with(|a| start_purge(a, Some(no_trash), None));
                }
            }
        }
    }
}

fn list_menu(hwnd: HWND, tab: usize) {
    let Some((rows, plan_mode)) =
        with(|a| (a.selected_rows(tab), a.plan.is_some() && a.busy.is_none()))
    else {
        return;
    };
    let Ok(menu) = (unsafe { CreatePopupMenu() }) else {
        return;
    };
    let mut pt = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut pt);
        let add = |id: u32, t: &str| {
            let _ = AppendMenuW(menu, MF_STRING, id as usize, &HSTRING::from(t));
        };
        let sep = || {
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        };
        if !rows.is_empty() {
            add(CM_OPEN, "開く(&O)");
            add(CM_LOCATE, "エクスプローラーで場所を開く(&L)");
            add(CM_EDITOR, "yyeditor で開く(&E)");
            add(CM_COPY_PATH, "パスをコピー(&C)");
        }
        match tab {
            TAB_SYNC if plan_mode && !rows.is_empty() => {
                sep();
                add(CM_OVERWRITE, "衝突: 上書きする(&W)");
                add(CM_KEEP_BOTH, "衝突: 両方残す(&B)");
                add(CM_SKIP, "送らない(&S)");
            }
            TAB_SEARCH => {
                sep();
                if rows.len() == 1 {
                    add(CM_VERSIONS, "このファイルの別の版を探す(&V)");
                    add(CM_SAME_CONTENT, "同じ中身のファイルを探す(&S)");
                    add(CM_SYNC_SRC, "このフォルダを同期の送り元にする(&Y)");
                    sep();
                }
                if !rows.is_empty() {
                    add(CM_TO_REVIEW, "削除の確認に加える(&R)");
                }
                add(CM_EXPORT, "一覧を CSV に書き出す(&X)...");
            }
            TAB_SIMILAR | TAB_DUPES if !rows.is_empty() => {
                sep();
                add(CM_TO_REVIEW, "このグループを削除の確認へ(&R)");
            }
            TAB_REVIEW if !rows.is_empty() => {
                sep();
                add(CM_CHECK, "チェックする(&K)");
                add(CM_UNCHECK, "チェックを外す(&U)");
                add(CM_REMOVE, "一覧から外す(&D)");
            }
            _ => {}
        }
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
        let cmd = cmd.0 as u32;
        let first = rows.first().copied();
        let path = first.and_then(|r| with(|a| a.row_path(tab, r)).flatten());
        match cmd {
            CM_OPEN => {
                if let Some((p, _)) = path {
                    open_with_shell(&p);
                }
            }
            CM_LOCATE => {
                if let Some((p, _)) = path
                    && let Err(e) = crate::app::workspacemode::open_explorer(&p)
                {
                    error_box(hwnd, &e);
                }
            }
            CM_EDITOR => {
                if let Some((p, line)) = path {
                    open_in_editor(hwnd, &p, line);
                }
            }
            CM_COPY_PATH => {
                let text: Vec<String> = rows
                    .iter()
                    .filter_map(|r| with(|a| a.row_path(tab, *r)).flatten())
                    .map(|(p, _)| p.to_string_lossy().into_owned())
                    .collect();
                let _ = crate::clipboard::set_text(hwnd, &text.join("\r\n"), false);
            }
            CM_EXPORT => export_search(hwnd),
            CM_SYNC_SRC => {
                if let Some((p, _)) = path
                    && let Some(dir) = p.parent()
                {
                    with(|a| {
                        set_text(a.edits.src, &dir.to_string_lossy());
                        a.plan = None;
                        a.show_tab(TAB_SYNC);
                        a.set_status(
                            "送り元にしました。送り先を指定して「比べる」を押してください",
                        );
                    });
                }
            }
            CM_VERSIONS | CM_SAME_CONTENT => {
                if let Some(r) = first {
                    find_related(r, cmd == CM_VERSIONS);
                }
            }
            CM_TO_REVIEW => to_review(tab, rows),
            CM_OVERWRITE | CM_KEEP_BOTH | CM_SKIP => {
                with(|a| {
                    let to = match cmd {
                        CM_OVERWRITE => Action::Update,
                        CM_KEEP_BOTH => Action::KeepBoth,
                        _ => Action::Same,
                    };
                    for r in &rows {
                        let Some(row) = a.sync_rows.get_mut(*r) else {
                            continue;
                        };
                        // 上書き・両方残すは衝突だけ。送らないはどれでも
                        if to != Action::Same && row.item.action != Action::Conflict {
                            continue;
                        }
                        row.item.action = to;
                        if let Some(p) = &mut a.plan
                            && let Some(it) = p.items.iter_mut().find(|i| i.rel == row.item.rel)
                        {
                            it.action = to;
                        }
                    }
                    a.refresh_list(TAB_SYNC);
                });
            }
            CM_CHECK | CM_UNCHECK => {
                with(|a| {
                    for r in &rows {
                        if let Some(c) = a.review.items.get_mut(*r)
                            && !c.keep
                        {
                            c.checked = cmd == CM_CHECK;
                        }
                    }
                    a.save_review();
                    a.refresh_list(TAB_REVIEW);
                    a.update_review_info();
                });
            }
            CM_REMOVE => {
                with(|a| {
                    let set: std::collections::HashSet<usize> = rows.iter().copied().collect();
                    let mut k = 0;
                    a.review.items.retain(|_| {
                        let keep = !set.contains(&k);
                        k += 1;
                        keep
                    });
                    a.save_review();
                    a.refresh_list(TAB_REVIEW);
                    a.update_review_info();
                });
            }
            _ => {}
        }
    }
}

/// ファイルを選ぶ（`save` に名前を渡したら保存の画面）。
fn pick_file(
    owner: HWND,
    filter: (&str, &str),
    folder: Option<&Path>,
    save: Option<&str>,
) -> Option<PathBuf> {
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree};
    use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
    use windows::Win32::UI::Shell::{
        FileOpenDialog, FileSaveDialog, IFileDialog, IFileOpenDialog, IFileSaveDialog, IShellItem,
        SHCreateItemFromParsingName, SIGDN_FILESYSPATH,
    };
    use windows::core::Interface;
    unsafe {
        let d: IFileDialog = if save.is_some() {
            CoCreateInstance::<_, IFileSaveDialog>(&FileSaveDialog, None, CLSCTX_INPROC_SERVER)
                .ok()?
                .cast()
                .ok()?
        } else {
            CoCreateInstance::<_, IFileOpenDialog>(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)
                .ok()?
                .cast()
                .ok()?
        };
        let name = HSTRING::from(filter.0);
        let spec = HSTRING::from(filter.1);
        let filters = [
            COMDLG_FILTERSPEC {
                pszName: PCWSTR(name.as_ptr()),
                pszSpec: PCWSTR(spec.as_ptr()),
            },
            COMDLG_FILTERSPEC {
                pszName: w!("すべてのファイル (*.*)"),
                pszSpec: w!("*.*"),
            },
        ];
        let _ = d.SetFileTypes(&filters);
        if let Some(ext) = filter.1.strip_prefix("*.") {
            let _ = d.SetDefaultExtension(&HSTRING::from(ext));
        }
        if let Some(f) = folder {
            let _ = std::fs::create_dir_all(f);
            if let Ok(item) =
                SHCreateItemFromParsingName::<_, _, IShellItem>(&HSTRING::from(f.as_os_str()), None)
            {
                let _ = d.SetFolder(&item);
            }
        }
        if let Some(n) = save {
            let _ = d.SetFileName(&HSTRING::from(n));
        }
        d.Show(Some(owner)).ok()?;
        let item = d.GetResult().ok()?;
        let p = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s.map(PathBuf::from)
    }
}

fn open_with_shell(p: &Path) {
    let w = HSTRING::from(p.as_os_str());
    unsafe {
        windows::Win32::UI::Shell::ShellExecuteW(None, w!("open"), &w, None, None, SW_SHOWNORMAL);
    }
}

/// yyeditor で開く（同じフォルダの yyeditor.exe。行の番号があればその行）。
fn open_in_editor(hwnd: HWND, p: &Path, line: u64) {
    let exe = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join("yyeditor.exe")))
        .filter(|e| e.is_file());
    let Some(exe) = exe else {
        error_box(hwnd, "yyeditor.exe が同じフォルダにありません。");
        return;
    };
    let mut cmd = std::process::Command::new(exe);
    if line > 0 {
        cmd.arg("--line").arg(line.to_string());
    }
    if let Err(e) = cmd.arg(p).spawn() {
        error_box(hwnd, &format!("yyeditor を起動できません: {e}"));
    }
}

/// 検索の結果を CSV に書き出す。
fn export_search(hwnd: HWND) {
    let Some(path) = pick_file(hwnd, ("CSV (*.csv)", "*.csv"), None, Some("検索結果.csv"))
    else {
        return;
    };
    let text = with(|a| {
        let mut s = String::from("\u{feff}名前,フォルダ,大きさ,更新日時,行,一致した行\r\n");
        for r in 0..a.search_rows.len() {
            let row: Vec<String> = (0..6)
                .map(|c| {
                    let v = if c == 2 {
                        let sr = &a.search_rows[r];
                        a.search_cats[sr.file.root].files[sr.file.index]
                            .meta
                            .size
                            .to_string()
                    } else {
                        a.cell(TAB_SEARCH, r, c)
                    };
                    if v.contains([',', '"', '\n']) {
                        format!("\"{}\"", v.replace('"', "\"\""))
                    } else {
                        v
                    }
                })
                .collect();
            s.push_str(&row.join(","));
            s.push_str("\r\n");
        }
        s
    })
    .unwrap_or_default();
    match std::fs::write(&path, text) {
        Ok(()) => {
            with(|a| a.set_status(&format!("書き出しました: {}", path.display()))).unwrap_or(())
        }
        Err(e) => error_box(hwnd, &format!("{}\n\n{e}", path.display())),
    }
}

fn command(id: u16, code: u32) {
    match id {
        ID_EXIT => {
            if let Some(f) = with(|a| a.frame) {
                unsafe {
                    let _ = PostMessageW(Some(f), WM_CLOSE, WPARAM(0), LPARAM(0));
                }
            }
        }
        _ if (ID_TAB_BASE..ID_TAB_BASE + 6).contains(&id) => {
            with(|a| a.show_tab((id - ID_TAB_BASE) as usize));
        }
        ID_HELP => {
            let _ = crate::help::show(None);
        }
        ID_OPEN_LOGS => {
            let dir = work::log_path().and_then(|p| p.parent().map(Path::to_path_buf));
            if let Some(d) = dir {
                let _ = std::fs::create_dir_all(&d);
                let _ = crate::app::workspacemode::open_explorer(&d);
            }
        }
        ID_CRASH_LOGS => crate::crash::open_log_dir(),
        ID_SETTINGS => {
            if let Some(p) = Config::default_path() {
                if !p.exists() {
                    if let Some(d) = p.parent() {
                        let _ = std::fs::create_dir_all(d);
                    }
                    let _ = std::fs::write(&p, Config::default_file_contents());
                }
                open_with_shell(&p);
            }
        }
        ID_ABOUT => {
            if let Some(f) = with(|a| a.frame) {
                info_box(
                    f,
                    &format!(
                        "yyfilemanager {}\n\n共有フォルダへのレジュームつき同期、ファイルの検索、似た名前の版と重複の検出、\
                         確かめてからの一括削除。",
                        env!("CARGO_PKG_VERSION")
                    ),
                );
            }
        }
        ID_JOB if code == CBN_SELCHANGE => {
            let name = with(|a| {
                let i = unsafe { SendMessageW(a.edits.job, CB_GETCURSEL, None, None).0 };
                if i < 0 {
                    return String::new();
                }
                a.jobs
                    .jobs
                    .get(i as usize)
                    .map(|j| j.name.clone())
                    .unwrap_or_default()
            })
            .unwrap_or_default();
            load_job(&name);
        }
        ID_JOB_SAVE => save_job(),
        ID_JOB_DELETE => delete_job(),
        ID_SWAP => {
            with(|a| {
                let (s, d) = (text_of(a.edits.src), text_of(a.edits.dst));
                set_text(a.edits.src, &d);
                set_text(a.edits.dst, &s);
                // 向きが変わったので計画は作り直す
                a.plan = None;
                a.sync_rows.clear();
                a.refresh_list(TAB_SYNC);
                set_text(
                    a.edits.sync_info,
                    "送り元と送り先を入れ替えました。「比べる」で確かめてください",
                );
            });
        }
        ID_SAVED if code == CBN_SELCHANGE => {
            let name = with(|a| {
                let i = unsafe { SendMessageW(a.edits.saved, CB_GETCURSEL, None, None).0 };
                (i >= 0)
                    .then(|| a.searches.searches.get(i as usize).map(|s| s.name.clone()))
                    .flatten()
                    .unwrap_or_default()
            })
            .unwrap_or_default();
            run_saved_search(&name);
        }
        ID_SAVED_SAVE => save_search(),
        ID_SAVED_DELETE => delete_search(),
        ID_EXPIRE_TRASH => expire_trash(),
        ID_VACUUM => vacuum_fulltext(),
        ID_SRC_BROWSE => {
            if let Some(h) = with(|a| a.edits.src) {
                browse_into(h, false);
            }
        }
        ID_DST_BROWSE => {
            if let Some(h) = with(|a| a.edits.dst) {
                browse_into(h, false);
            }
        }
        ID_SEARCH_BROWSE => {
            if let Some(h) = with(|a| a.edits.search_roots) {
                browse_into(h, true);
            }
        }
        ID_SIM_BROWSE => {
            if let Some(h) = with(|a| a.edits.sim_roots) {
                browse_into(h, true);
            }
        }
        ID_DUP_BROWSE => {
            if let Some(h) = with(|a| a.edits.dup_roots) {
                browse_into(h, true);
            }
        }
        ID_PLAN => cmd_plan(),
        ID_RUN => cmd_run(false),
        ID_RESUME => cmd_run(true),
        ID_SEARCH => cmd_search(),
        ID_SIM_FIND => cmd_similar(),
        ID_DUP_FIND => cmd_dupes(),
        ID_SIM_TO_REVIEW => to_review(TAB_SIMILAR, Vec::new()),
        ID_DUP_TO_REVIEW => to_review(TAB_DUPES, Vec::new()),
        ID_CHECK_ALL | ID_UNCHECK_ALL => {
            with(|a| {
                for c in &mut a.review.items {
                    if !c.keep {
                        c.checked = id == ID_CHECK_ALL;
                    }
                }
                a.save_review();
                a.refresh_list(TAB_REVIEW);
                a.update_review_info();
            });
        }
        ID_PURGE => cmd_purge(),
        ID_UNDO => cmd_undo(),
        ID_CLEAR_REVIEW => {
            with(|a| {
                a.review.items.clear();
                a.save_review();
                a.refresh_list(TAB_REVIEW);
                a.update_review_info();
            });
        }
        ID_CANCEL => cancel(),
        _ => {}
    }
}

fn cancel() {
    with(|a| {
        if let Some((c, w)) = &a.busy {
            c.store(true, Ordering::Relaxed);
            let msg = format!("「{w}」を止めています…");
            a.set_status(&msg);
        }
    });
}

fn key_hook(msg: &MSG) -> bool {
    let vk = VIRTUAL_KEY(msg.wParam.0 as u16);
    let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
    if vk == VK_F1 && !crate::help::contains(msg.hwnd) {
        command(ID_HELP, 0);
        return true;
    }
    if vk == VK_ESCAPE && with(|a| a.busy.is_some()) == Some(true) {
        cancel();
        return true;
    }
    if ctrl && (0x31..=0x36).contains(&vk.0) {
        command(ID_TAB_BASE + (vk.0 - 0x31), 0);
        return true;
    }
    if ctrl && vk == VK_F {
        with(|a| {
            a.show_tab(TAB_SEARCH);
            unsafe {
                let _ = SetFocus(Some(a.edits.query));
            }
        });
        return true;
    }
    // 検索欄で Enter
    if vk == VK_RETURN && with(|a| msg.hwnd == a.edits.query) == Some(true) {
        cmd_search();
        return true;
    }
    false
}

/// 一覧の WM_NOTIFY。
fn on_list_notify(hwnd: HWND, tab: usize, hdr: &NMHDR, lparam: LPARAM) -> LRESULT {
    match hdr.code {
        LVN_GETDISPINFOW => {
            let nm = unsafe { &mut *(lparam.0 as *mut NMLVDISPINFOW) };
            let row = nm.item.iItem.max(0) as usize;
            if nm.item.mask & LVIF_TEXT != LVIF_TEXT_ZERO
                && nm.item.cchTextMax > 0
                && !nm.item.pszText.is_null()
            {
                let text = with(|a| a.cell(tab, row, nm.item.iSubItem.max(0) as usize))
                    .unwrap_or_default();
                let buf = unsafe {
                    std::slice::from_raw_parts_mut(nm.item.pszText.0, nm.item.cchTextMax as usize)
                };
                let mut n = 0;
                for u in text.encode_utf16().take(buf.len() - 1) {
                    buf[n] = u;
                    n += 1;
                }
                buf[n] = 0;
            }
            if tab == TAB_REVIEW && nm.item.mask & LVIF_STATE != LVIF_STATE_ZERO {
                let checked = with(|a| {
                    a.review
                        .items
                        .get(row)
                        .is_some_and(|c| c.checked && !c.keep)
                })
                .unwrap_or(false);
                // 状態の絵（1: チェックなし、2: チェックあり）
                nm.item.state = LIST_VIEW_ITEM_STATE_FLAGS(((checked as u32) + 1) << 12);
                nm.item.stateMask = LVIS_STATEIMAGEMASK;
            }
            LRESULT(0)
        }
        NM_CLICK if tab == TAB_REVIEW => {
            let nm = unsafe { &*(lparam.0 as *const NMITEMACTIVATE) };
            let mut hit = LVHITTESTINFO {
                pt: nm.ptAction,
                ..Default::default()
            };
            let item = unsafe {
                SendMessageW(
                    hdr.hwndFrom,
                    LVM_HITTEST,
                    None,
                    Some(LPARAM(&mut hit as *mut _ as isize)),
                )
                .0
            };
            if item >= 0 && hit.flags & LVHT_ONITEMSTATEICON == LVHT_ONITEMSTATEICON {
                toggle_review(&[item as usize]);
            }
            LRESULT(0)
        }
        LVN_KEYDOWN if tab == TAB_REVIEW => {
            let nm = unsafe { &*(lparam.0 as *const NMLVKEYDOWN) };
            if nm.wVKey == VK_SPACE.0 {
                let rows = with(|a| a.selected_rows(TAB_REVIEW)).unwrap_or_default();
                toggle_review(&rows);
            }
            LRESULT(0)
        }
        NM_DBLCLK => {
            let nm = unsafe { &*(lparam.0 as *const NMITEMACTIVATE) };
            if nm.iItem >= 0
                && let Some(Some((p, line))) = with(|a| a.row_path(tab, nm.iItem as usize))
            {
                if line > 0 {
                    open_in_editor(hwnd, &p, line);
                } else {
                    open_with_shell(&p);
                }
            }
            LRESULT(0)
        }
        NM_RCLICK => {
            list_menu(hwnd, tab);
            LRESULT(1)
        }
        _ => LRESULT(0),
    }
}

const LVIF_TEXT_ZERO: LIST_VIEW_ITEM_FLAGS = LIST_VIEW_ITEM_FLAGS(0);
const LVIF_STATE_ZERO: LIST_VIEW_ITEM_FLAGS = LIST_VIEW_ITEM_FLAGS(0);

fn toggle_review(rows: &[usize]) {
    with(|a| {
        for r in rows {
            if let Some(c) = a.review.items.get_mut(*r)
                && !c.keep
            {
                c.checked = !c.checked;
            }
        }
        a.save_review();
        a.refresh_list(TAB_REVIEW);
        a.update_review_info();
    });
}

extern "system" fn frame_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_SIZE => {
            with(|a| a.layout());
            LRESULT(0)
        }
        WM_COMMAND => {
            command(loword(wparam.0) as u16, hiword(wparam.0));
            LRESULT(0)
        }
        WM_NOTIFY => {
            let hdr = unsafe { &*(lparam.0 as *const NMHDR) };
            if hdr.idFrom == ID_TABS as usize && hdr.code == TCN_SELCHANGE {
                let i = unsafe { SendMessageW(hdr.hwndFrom, TCM_GETCURSEL, None, None).0 };
                if i >= 0 {
                    with(|a| a.show_tab(i as usize));
                }
                return LRESULT(0);
            }
            let id = hdr.idFrom as u16;
            if (ID_LIST_BASE..ID_LIST_BASE + 5).contains(&id) {
                return on_list_notify(hwnd, (id - ID_LIST_BASE) as usize, hdr, lparam);
            }
            LRESULT(0)
        }
        WM_DROPFILES => {
            // フォルダを落とすと、今のタブの場所の欄に足す
            let drop = windows::Win32::UI::Shell::HDROP(wparam.0 as *mut _);
            let mut paths = Vec::new();
            unsafe {
                let n = windows::Win32::UI::Shell::DragQueryFileW(drop, u32::MAX, None);
                for i in 0..n {
                    let mut buf = [0u16; 1024];
                    let len =
                        windows::Win32::UI::Shell::DragQueryFileW(drop, i, Some(&mut buf)) as usize;
                    paths.push(String::from_utf16_lossy(&buf[..len]));
                }
                windows::Win32::UI::Shell::DragFinish(drop);
            }
            with(|a| {
                let target = match a.current {
                    TAB_SYNC => Some(a.edits.src),
                    TAB_SEARCH => Some(a.edits.search_roots),
                    TAB_SIMILAR => Some(a.edits.sim_roots),
                    TAB_DUPES => Some(a.edits.dup_roots),
                    _ => None,
                };
                if let Some(h) = target {
                    let cur = text_of(h);
                    let mut parts: Vec<String> = if a.current == TAB_SYNC {
                        Vec::new()
                    } else {
                        roots_of(&cur)
                            .into_iter()
                            .map(|p| p.to_string_lossy().into_owned())
                            .collect()
                    };
                    parts.extend(paths.into_iter().take(if a.current == TAB_SYNC {
                        1
                    } else {
                        usize::MAX
                    }));
                    set_text(h, &parts.join("; "));
                }
            });
            LRESULT(0)
        }
        WM_CLOSE => {
            let busy = with(|a| a.busy.as_ref().map(|(_, w)| w.clone())).flatten();
            if let Some(w) = busy {
                let r = unsafe {
                    MessageBoxW(
                        Some(hwnd),
                        &HSTRING::from(format!(
                            "「{w}」の途中です。止めて終了しますか？（同期は次に起動したときに続けられます）"
                        )),
                        w!("yyfilemanager"),
                        MB_OKCANCEL | MB_ICONQUESTION,
                    )
                };
                if r != IDOK {
                    return LRESULT(0);
                }
                cancel();
            }
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        WM_APP_FM => {
            on_messages();
            LRESULT(0)
        }
        _ => default_proc(hwnd, msg, wparam, lparam),
    }
}
