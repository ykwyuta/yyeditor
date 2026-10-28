//! アドレスバーの下の候補（19 章 4.2）: 入力した文字を URL か題名に含む閲覧履歴のページを、最後に見たのが
//! 新しい順に並べる（`yy_browser::history::suggest`）。文字を足すほど候補は減る。
//!
//! 候補の一覧はフォーカスを取らないポップアップのリスト（`WS_EX_NOACTIVATE`）。↓・↑ で選び、Enter で開く
//! （選んでいなければ入力した文字で開く・検索する）。クリックでも開く。Esc・アドレスバーから離れると閉じる。

use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::HFONT;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, w};

/// 候補の数の上限
pub(super) const MAX: usize = 10;

/// 候補の一覧を作る（隠しておく）。通知（`LBN_SELCHANGE`）は `frame` の `WM_COMMAND` に来る。
pub(super) fn create(
    frame: HWND,
    instance: windows::Win32::Foundation::HINSTANCE,
    font: HFONT,
) -> HWND {
    unsafe {
        let h = CreateWindowExW(
            WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            w!("LISTBOX"),
            None,
            WS_POPUP
                | WS_BORDER
                | WS_VSCROLL
                | WINDOW_STYLE((LBS_NOTIFY | LBS_NOINTEGRALHEIGHT) as u32),
            0,
            0,
            10,
            10,
            Some(frame),
            None,
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
    }
}

/// 一覧の 1 行（題名 — URL。題名がなければ URL）。
pub(super) fn row(title: &str, url: &str) -> String {
    let title: String = title.trim().chars().take(80).collect();
    if title.is_empty() {
        url.to_owned()
    } else {
        format!("{title}　—　{url}")
    }
}

/// 候補を `address`（アドレスバー）のすぐ下に出す。空なら隠す。
pub(super) fn show(popup: HWND, address: HWND, rows: &[String]) {
    if rows.is_empty() {
        hide(popup);
        return;
    }
    unsafe {
        SendMessageW(popup, WM_SETREDRAW, Some(WPARAM(0)), None);
        SendMessageW(popup, LB_RESETCONTENT, None, None);
        for r in rows {
            let s = HSTRING::from(r.as_str());
            SendMessageW(popup, LB_ADDSTRING, None, Some(LPARAM(s.as_ptr() as isize)));
        }
        SendMessageW(popup, WM_SETREDRAW, Some(WPARAM(1)), None);
        let item_h = SendMessageW(popup, LB_GETITEMHEIGHT, None, None).0.max(12) as i32;
        let mut rc = RECT::default();
        let _ = GetWindowRect(address, &mut rc);
        let height = item_h * rows.len() as i32 + 4;
        let _ = SetWindowPos(
            popup,
            Some(HWND_TOPMOST),
            rc.left,
            rc.bottom,
            (rc.right - rc.left).max(360),
            height,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
        let _ = windows::Win32::Graphics::Gdi::InvalidateRect(Some(popup), None, true);
    }
}

pub(super) fn hide(popup: HWND) {
    unsafe {
        if IsWindowVisible(popup).as_bool() {
            let _ = ShowWindow(popup, SW_HIDE);
        }
    }
}

pub(super) fn visible(popup: HWND) -> bool {
    unsafe { IsWindowVisible(popup).as_bool() }
}

/// 選んでいる行（なければ `None`）。
pub(super) fn selected(popup: HWND) -> Option<usize> {
    let i = unsafe { SendMessageW(popup, LB_GETCURSEL, None, None).0 };
    (i >= 0).then_some(i as usize)
}

/// 選ぶ行を `delta` だけ動かす（端では止まる。↑ で一番上より上へ行けば選ばない状態に戻す）。
pub(super) fn move_selection(popup: HWND, delta: i32) {
    let n = unsafe { SendMessageW(popup, LB_GETCOUNT, None, None).0 } as i32;
    if n <= 0 {
        return;
    }
    let cur = selected(popup).map_or(-1, |i| i as i32);
    let next = (cur + delta).clamp(-1, n - 1);
    unsafe {
        SendMessageW(
            popup,
            LB_SETCURSEL,
            Some(WPARAM(next as isize as usize)),
            None,
        );
    }
}
