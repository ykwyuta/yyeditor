//! 閲覧履歴の画面と、閲覧データの消去の画面（19 章 4.2）。

use windows::Win32::Foundation::{FILETIME, HWND, LPARAM, SYSTEMTIME, WPARAM};
use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
use windows::Win32::UI::Controls::{
    BST_CHECKED, CheckDlgButton, EM_SETCUEBANNER, IsDlgButtonChecked,
};
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, w};
use yy_browser::history::{self, Visit};

use crate::goto::{CLASS_BUTTON, CLASS_EDIT, CLASS_STATIC, Template};

const CLASS_LISTBOX: u16 = 0x0083;
const CLASS_COMBO: u16 = 0x0085;
const IDOK_: u16 = 1;
const IDCANCEL_: u16 = 2;
const NO_ID: u16 = 0xFFFF;
/// 一覧に出す件数の上限（多すぎると一覧が重い）。
const LIST_MAX: usize = 5000;

/// UNIX 時間を手元の時刻の `2026-10-09 14:23` に。
pub(super) fn local_time(t: u64) -> String {
    // 1601-01-01 からの 100 ナノ秒
    let ft = (t + 11_644_473_600) * 10_000_000;
    let ft = FILETIME {
        dwLowDateTime: ft as u32,
        dwHighDateTime: (ft >> 32) as u32,
    };
    let mut utc = SYSTEMTIME::default();
    let mut local = SYSTEMTIME::default();
    unsafe {
        if FileTimeToSystemTime(&ft, &mut utc).is_err()
            || SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).is_err()
        {
            return t.to_string();
        }
    }
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        local.wYear, local.wMonth, local.wDay, local.wHour, local.wMinute
    )
}

// ---- 閲覧履歴 ------------------------------------------------------------------------------

const H_SEARCH: u16 = 100;
const H_LIST: u16 = 101;
const H_OPEN: u16 = 102;
const H_OPEN_TAB: u16 = 103;
const H_DELETE: u16 = 104;
const H_CLEAR: u16 = 105;
const H_COUNT: u16 = 106;

/// 履歴の画面の結果。
pub(super) enum HistoryResult {
    Open(String, bool),
    /// 閲覧データの消去の画面を開く
    Clear,
    None,
}

struct HState {
    path: std::path::PathBuf,
    all: Vec<Visit>,
    /// 一覧に出しているもの（`all` の番号）
    shown: Vec<usize>,
    result: HistoryResult,
}

/// 閲覧履歴の画面（`path` はこのプロファイルの履歴のファイル）。
pub(super) fn show(owner: HWND, path: std::path::PathBuf) -> HistoryResult {
    let mut t = Template::dialog("閲覧履歴", 460, 260);
    t.item(
        (WS_BORDER | WS_TABSTOP).0 | ES_AUTOHSCROLL as u32,
        7,
        7,
        374,
        13,
        H_SEARCH,
        CLASS_EDIT,
        "",
    );
    t.item(
        (WS_BORDER | WS_TABSTOP | WS_VSCROLL | WS_HSCROLL).0
            | (LBS_NOTIFY | LBS_NOINTEGRALHEIGHT | LBS_EXTENDEDSEL) as u32,
        7,
        24,
        374,
        208,
        H_LIST,
        CLASS_LISTBOX,
        "",
    );
    let mut y = 24;
    for (id, text) in [
        (H_OPEN, "開く"),
        (H_OPEN_TAB, "新しいタブで開く"),
        (H_DELETE, "選んだものを消す"),
        (H_CLEAR, "閲覧データの消去..."),
    ] {
        t.item(
            WS_TABSTOP.0 | BS_PUSHBUTTON as u32,
            387,
            y,
            66,
            14,
            id,
            CLASS_BUTTON,
            text,
        );
        y += 17;
    }
    t.item(0, 7, 240, 300, 10, H_COUNT, CLASS_STATIC, "");
    t.item(
        WS_TABSTOP.0 | BS_PUSHBUTTON as u32,
        403,
        238,
        50,
        14,
        IDCANCEL_,
        CLASS_BUTTON,
        "閉じる",
    );
    let aligned = t.aligned();
    let mut st = HState {
        all: history::load(&path),
        path,
        shown: Vec::new(),
        result: HistoryResult::None,
    };
    unsafe {
        DialogBoxIndirectParamW(
            None,
            aligned.as_ptr() as *const DLGTEMPLATE,
            Some(owner),
            Some(history_proc),
            LPARAM(&mut st as *mut HState as isize),
        );
    }
    st.result
}

fn get_text(dlg: HWND, id: u16) -> String {
    unsafe {
        let h = GetDlgItem(Some(dlg), id as i32).unwrap_or_default();
        let n = GetWindowTextLengthW(h);
        let mut buf = vec![0u16; n as usize + 1];
        let got = GetWindowTextW(h, &mut buf) as usize;
        String::from_utf16_lossy(&buf[..got])
    }
}

fn fill(dlg: HWND, st: &mut HState) {
    let q = get_text(dlg, H_SEARCH);
    st.shown = history::search_visit_indices(&st.all, &q);
    st.shown.truncate(LIST_MAX);
    unsafe {
        SendDlgItemMessageW(dlg, H_LIST as i32, WM_SETREDRAW, WPARAM(0), LPARAM(0));
        SendDlgItemMessageW(dlg, H_LIST as i32, LB_RESETCONTENT, WPARAM(0), LPARAM(0));
        let mut widest = 0;
        for &i in &st.shown {
            let v = &st.all[i];
            let title = if v.title.trim().is_empty() {
                "（題名なし）"
            } else {
                v.title.as_str()
            };
            let s = format!("{}　{}　—　{}", local_time(v.time), title, v.url);
            widest = widest.max(s.chars().count());
            let s = HSTRING::from(s);
            SendDlgItemMessageW(
                dlg,
                H_LIST as i32,
                LB_ADDSTRING,
                WPARAM(0),
                LPARAM(s.as_ptr() as isize),
            );
        }
        SendDlgItemMessageW(
            dlg,
            H_LIST as i32,
            LB_SETHORIZONTALEXTENT,
            WPARAM(widest * 14),
            LPARAM(0),
        );
        SendDlgItemMessageW(dlg, H_LIST as i32, WM_SETREDRAW, WPARAM(1), LPARAM(0));
        let _ = windows::Win32::Graphics::Gdi::InvalidateRect(
            Some(GetDlgItem(Some(dlg), H_LIST as i32).unwrap_or_default()),
            None,
            true,
        );
    }
    let more = if st.shown.len() >= LIST_MAX {
        format!("（新しい {LIST_MAX} 件だけ出しています。語で絞ってください）")
    } else {
        String::new()
    };
    unsafe {
        let _ = SetDlgItemTextW(
            dlg,
            H_COUNT as i32,
            &HSTRING::from(format!(
                "{} 件 / 全部で {} 件{more}",
                st.shown.len(),
                st.all.len()
            )),
        );
    }
}

/// 選んでいる行（`all` の番号。複数選べる）。
fn selection(dlg: HWND, st: &HState) -> Vec<usize> {
    unsafe {
        let n = SendDlgItemMessageW(dlg, H_LIST as i32, LB_GETSELCOUNT, WPARAM(0), LPARAM(0)).0;
        if n <= 0 {
            return Vec::new();
        }
        let mut idx = vec![0i32; n as usize];
        let got = SendDlgItemMessageW(
            dlg,
            H_LIST as i32,
            LB_GETSELITEMS,
            WPARAM(n as usize),
            LPARAM(idx.as_mut_ptr() as isize),
        )
        .0
        .max(0) as usize;
        idx[..got.min(idx.len())]
            .iter()
            .filter_map(|&i| st.shown.get(i as usize).copied())
            .collect()
    }
}

extern "system" fn history_proc(dlg: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> isize {
    unsafe {
        match msg {
            WM_INITDIALOG => {
                SetWindowLongPtrW(dlg, GWLP_USERDATA, lparam.0);
                let st = &mut *(lparam.0 as *mut HState);
                let cue = HSTRING::from("題名・URL で絞り込む");
                SendDlgItemMessageW(
                    dlg,
                    H_SEARCH as i32,
                    EM_SETCUEBANNER,
                    WPARAM(1),
                    LPARAM(cue.as_ptr() as isize),
                );
                fill(dlg, st);
                if let Ok(h) = GetDlgItem(Some(dlg), H_SEARCH as i32) {
                    let _ = SetFocus(Some(h));
                }
                0
            }
            WM_COMMAND => {
                let st = &mut *(GetWindowLongPtrW(dlg, GWLP_USERDATA) as *mut HState);
                let id = (wparam.0 & 0xffff) as u16;
                let code = ((wparam.0 >> 16) & 0xffff) as u32;
                match id {
                    H_SEARCH if code == EN_CHANGE => fill(dlg, st),
                    H_OPEN | H_OPEN_TAB => {
                        if let Some(&i) = selection(dlg, st).first() {
                            st.result =
                                HistoryResult::Open(st.all[i].url.clone(), id == H_OPEN_TAB);
                            let _ = EndDialog(dlg, IDOK_ as isize);
                        }
                    }
                    H_LIST if code == LBN_DBLCLK => {
                        if let Some(&i) = selection(dlg, st).first() {
                            st.result = HistoryResult::Open(st.all[i].url.clone(), false);
                            let _ = EndDialog(dlg, IDOK_ as isize);
                        }
                    }
                    H_DELETE => {
                        let mut sel = selection(dlg, st);
                        if !sel.is_empty() {
                            sel.sort_unstable();
                            for i in sel.into_iter().rev() {
                                st.all.remove(i);
                            }
                            if let Err(e) = history::save(&st.path, &st.all) {
                                crate::util::error_box(dlg, &format!("保存できません: {e}"));
                            }
                            fill(dlg, st);
                        }
                    }
                    H_CLEAR => {
                        st.result = HistoryResult::Clear;
                        let _ = EndDialog(dlg, IDOK_ as isize);
                    }
                    IDCANCEL_ => {
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

// ---- 閲覧データの消去 ----------------------------------------------------------------------

const C_PERIOD: u16 = 200;
const C_HISTORY: u16 = 201;
const C_DOWNLOADS: u16 = 202;
const C_COOKIES: u16 = 203;
const C_CACHE: u16 = 204;
const C_AUTOFILL: u16 = 205;

/// 消す期間（秒。`None` はすべて）。
const PERIODS: [(&str, Option<u64>); 5] = [
    ("過去 1 時間", Some(3600)),
    ("過去 24 時間", Some(86_400)),
    ("過去 7 日間", Some(7 * 86_400)),
    ("過去 4 週間", Some(28 * 86_400)),
    ("すべての期間", None),
];

/// 消すもの。
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct ClearRequest {
    /// この秒数だけ前から（`None` はすべて）
    pub within: Option<u64>,
    pub history: bool,
    pub downloads: bool,
    pub cookies: bool,
    pub cache: bool,
    pub autofill: bool,
}

struct CState {
    result: Option<ClearRequest>,
}

/// 閲覧データの消去の画面。
pub(super) fn clear_dialog(owner: HWND) -> Option<ClearRequest> {
    let mut t = Template::dialog("閲覧データの消去", 260, 152);
    t.item(0, 7, 9, 50, 10, NO_ID, CLASS_STATIC, "期間");
    t.item(
        (WS_TABSTOP | WS_VSCROLL).0 | CBS_DROPDOWNLIST as u32,
        60,
        7,
        193,
        100,
        C_PERIOD,
        CLASS_COMBO,
        "",
    );
    let mut y = 28;
    for (id, text) in [
        (C_HISTORY, "閲覧履歴"),
        (C_DOWNLOADS, "ダウンロード履歴（ファイルは消しません）"),
        (C_CACHE, "キャッシュされた画像とファイル"),
        (C_COOKIES, "Cookie とサイトのデータ（ログインが外れます）"),
        (C_AUTOFILL, "自動入力のデータとパスワード"),
    ] {
        t.item(
            WS_TABSTOP.0 | BS_AUTOCHECKBOX as u32,
            15,
            y,
            238,
            12,
            id,
            CLASS_BUTTON,
            text,
        );
        y += 16;
    }
    t.item(
        0,
        7,
        112,
        246,
        10,
        NO_ID,
        CLASS_STATIC,
        "このプロファイルのデータだけを消します。",
    );
    t.item(
        WS_TABSTOP.0 | BS_DEFPUSHBUTTON as u32,
        149,
        131,
        50,
        14,
        IDOK_,
        CLASS_BUTTON,
        "消去",
    );
    t.item(
        WS_TABSTOP.0 | BS_PUSHBUTTON as u32,
        203,
        131,
        50,
        14,
        IDCANCEL_,
        CLASS_BUTTON,
        "キャンセル",
    );
    let aligned = t.aligned();
    let mut st = CState { result: None };
    unsafe {
        DialogBoxIndirectParamW(
            None,
            aligned.as_ptr() as *const DLGTEMPLATE,
            Some(owner),
            Some(clear_proc),
            LPARAM(&mut st as *mut CState as isize),
        );
    }
    st.result
}

extern "system" fn clear_proc(dlg: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> isize {
    unsafe {
        match msg {
            WM_INITDIALOG => {
                SetWindowLongPtrW(dlg, GWLP_USERDATA, lparam.0);
                for (label, _) in PERIODS {
                    let s = HSTRING::from(label);
                    SendDlgItemMessageW(
                        dlg,
                        C_PERIOD as i32,
                        CB_ADDSTRING,
                        WPARAM(0),
                        LPARAM(s.as_ptr() as isize),
                    );
                }
                SendDlgItemMessageW(dlg, C_PERIOD as i32, CB_SETCURSEL, WPARAM(1), LPARAM(0));
                for id in [C_HISTORY, C_DOWNLOADS, C_CACHE] {
                    let _ = CheckDlgButton(dlg, id as i32, BST_CHECKED);
                }
                1
            }
            WM_COMMAND => {
                let st = &mut *(GetWindowLongPtrW(dlg, GWLP_USERDATA) as *mut CState);
                match (wparam.0 & 0xffff) as u16 {
                    IDOK_ => {
                        let on = |id: u16| IsDlgButtonChecked(dlg, id as i32) == BST_CHECKED.0;
                        let sel = SendDlgItemMessageW(
                            dlg,
                            C_PERIOD as i32,
                            CB_GETCURSEL,
                            WPARAM(0),
                            LPARAM(0),
                        )
                        .0;
                        let r = ClearRequest {
                            within: PERIODS.get(sel.max(0) as usize).and_then(|p| p.1),
                            history: on(C_HISTORY),
                            downloads: on(C_DOWNLOADS),
                            cookies: on(C_COOKIES),
                            cache: on(C_CACHE),
                            autofill: on(C_AUTOFILL),
                        };
                        if !(r.history || r.downloads || r.cookies || r.cache || r.autofill) {
                            MessageBoxW(
                                Some(dlg),
                                w!("消すものを選んでください。"),
                                w!("yybrowser"),
                                MB_OK | MB_ICONINFORMATION,
                            );
                            return 1;
                        }
                        st.result = Some(r);
                        let _ = EndDialog(dlg, IDOK_ as isize);
                    }
                    IDCANCEL_ => {
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
