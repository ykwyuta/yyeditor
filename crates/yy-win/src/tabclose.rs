//! タブの閉じるボタン。
//!
//! 標準のタブコントロールには閉じるボタンがないため、サブクラス化して各タブの右端に
//! 「×」を描き、そこのクリック（と中ボタンのクリック）でフレームにタブを閉じるよう知らせる。
//! 「×」の場所はタブの文字列の後ろに空白を足して空けておく（[`LABEL_PAD`]）。
//!
//! タブの右クリックでは、フレームに [`WM_APP_TAB_MENU`] を送る。フレームは [`menu`] で
//! 「閉じる・ほかのタブを閉じる・右側のタブを閉じる・左側のタブを閉じる」のメニューを出す
//! （エディタとターミナルで共通）。

use std::cell::Cell;

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreatePen, CreateSolidBrush, DeleteObject, FillRect, GetDC, InvalidateRect, LineTo, MoveToEx,
    PS_SOLID, ReleaseDC, SelectObject,
};
use windows::Win32::UI::Controls::{
    TCHITTESTINFO, TCM_GETITEMCOUNT, TCM_GETITEMRECT, TCM_HITTEST, WM_MOUSELEAVE,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::*;

/// タブを閉じる要求（`wparam` がタブの番号）。フレームに送る。
pub(crate) const WM_APP_CLOSE_TAB: u32 = WM_APP + 20;

/// タブの右クリック（`wparam` がタブの番号）。フレームに送る。
pub(crate) const WM_APP_TAB_MENU: u32 = WM_APP + 24;

/// タブの右クリックのメニューで選んだもの。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TabMenu {
    /// このタブを閉じる
    Close,
    /// このタブ以外を閉じる
    Others,
    /// このタブより右側を閉じる
    Right,
    /// このタブより左側を閉じる
    Left,
}

impl TabMenu {
    /// 閉じるタブの番号（小さい順）。`index` はメニューを出したタブ、`count` はタブの数。
    pub(crate) fn targets(self, index: usize, count: usize) -> Vec<usize> {
        if index >= count {
            return Vec::new();
        }
        match self {
            TabMenu::Close => vec![index],
            TabMenu::Others => (0..count).filter(|&i| i != index).collect(),
            TabMenu::Right => (index + 1..count).collect(),
            TabMenu::Left => (0..index).collect(),
        }
    }
}

/// タブ `index` の右クリックのメニューを出して、選んだものを返す（`count` はタブの数）。
pub(crate) fn menu(owner: HWND, index: usize, count: usize) -> Option<TabMenu> {
    const CLOSE: usize = 1;
    const OTHERS: usize = 2;
    const RIGHT: usize = 3;
    const LEFT: usize = 4;
    unsafe {
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let m = CreatePopupMenu().ok()?;
        let item = |id: usize, text: windows::core::PCWSTR, on: bool| {
            let flags = if on { MF_STRING } else { MF_STRING | MF_GRAYED };
            let _ = AppendMenuW(m, flags, id, text);
        };
        item(CLOSE, windows::core::w!("閉じる(&C)"), true);
        let _ = AppendMenuW(m, MF_SEPARATOR, 0, None);
        item(
            OTHERS,
            windows::core::w!("ほかのタブをすべて閉じる(&O)"),
            count > 1,
        );
        item(
            RIGHT,
            windows::core::w!("右側のタブを閉じる(&R)"),
            index + 1 < count,
        );
        item(LEFT, windows::core::w!("左側のタブを閉じる(&L)"), index > 0);
        let cmd = TrackPopupMenu(
            m,
            TPM_RETURNCMD | TPM_RIGHTBUTTON,
            pt.x,
            pt.y,
            None,
            owner,
            None,
        );
        let _ = DestroyMenu(m);
        match cmd.0 as usize {
            CLOSE => Some(TabMenu::Close),
            OTHERS => Some(TabMenu::Others),
            RIGHT => Some(TabMenu::Right),
            LEFT => Some(TabMenu::Left),
            _ => None,
        }
    }
}

/// 「×」の場所を空けるためにタブの文字列の後ろに足す空白。
pub(crate) const LABEL_PAD: &str = "\u{3000}\u{3000}";

const SUBCLASS_ID: usize = 1;

thread_local! {
    /// マウスが上にある「×」
    static HOVER: Cell<Option<usize>> = const { Cell::new(None) };
    /// 押した「×」（離したときに同じ「×」の上なら閉じる）
    static PRESSED: Cell<Option<usize>> = const { Cell::new(None) };
}

/// タブコントロールに閉じるボタンを付ける。`frame` に [`WM_APP_CLOSE_TAB`] を送る。
pub(crate) fn install(tabbar: HWND, frame: HWND) {
    unsafe {
        let _ = SetWindowSubclass(tabbar, Some(subclass_proc), SUBCLASS_ID, frame.0 as usize);
    }
}

fn scale(v: i32, dpi: u32) -> i32 {
    v * dpi as i32 / 96
}

/// タブ `i` の「×」の矩形。
fn close_rect(tab: HWND, i: usize) -> Option<RECT> {
    let mut r = RECT::default();
    let ok = unsafe {
        SendMessageW(
            tab,
            TCM_GETITEMRECT,
            Some(WPARAM(i)),
            Some(LPARAM(&mut r as *mut RECT as isize)),
        )
    };
    if ok.0 == 0 {
        return None;
    }
    let dpi = unsafe { GetDpiForWindow(tab) }.max(96);
    let size = scale(14, dpi);
    let right = r.right - scale(6, dpi);
    let top = (r.top + r.bottom - size) / 2;
    Some(RECT {
        left: right - size,
        top,
        right,
        bottom: top + size,
    })
}

fn count(tab: HWND) -> usize {
    unsafe { SendMessageW(tab, TCM_GETITEMCOUNT, None, None).0.max(0) as usize }
}

/// 位置の「×」のタブの番号。
fn hit_close(tab: HWND, lparam: LPARAM) -> Option<usize> {
    let (x, y) = (
        (lparam.0 & 0xFFFF) as i16 as i32,
        ((lparam.0 >> 16) & 0xFFFF) as i16 as i32,
    );
    (0..count(tab)).find(|&i| {
        close_rect(tab, i).is_some_and(|r| x >= r.left && x < r.right && y >= r.top && y < r.bottom)
    })
}

/// 位置のタブの番号。
fn hit_tab(tab: HWND, lparam: LPARAM) -> Option<usize> {
    let mut info = TCHITTESTINFO {
        pt: POINT {
            x: (lparam.0 & 0xFFFF) as i16 as i32,
            y: ((lparam.0 >> 16) & 0xFFFF) as i16 as i32,
        },
        ..Default::default()
    };
    let i = unsafe {
        SendMessageW(
            tab,
            TCM_HITTEST,
            None,
            Some(LPARAM(&mut info as *mut TCHITTESTINFO as isize)),
        )
    };
    (i.0 >= 0).then_some(i.0 as usize)
}

/// 各タブの「×」を描く（マウスが上にあれば背景を付ける）。
fn draw(tab: HWND) {
    let dpi = unsafe { GetDpiForWindow(tab) }.max(96);
    let hover = HOVER.with(|h| h.get());
    let pressed = PRESSED.with(|p| p.get());
    unsafe {
        let dc = GetDC(Some(tab));
        let pen = CreatePen(PS_SOLID, scale(1, dpi).max(1), COLORREF(0x0050_5050));
        let old = SelectObject(dc, pen.into());
        for i in 0..count(tab) {
            let Some(r) = close_rect(tab, i) else {
                continue;
            };
            if hover == Some(i) {
                let color = if pressed == Some(i) {
                    COLORREF(0x00B0_B0B0)
                } else {
                    COLORREF(0x00D8_D8D8)
                };
                let brush = CreateSolidBrush(color);
                FillRect(dc, &r, brush);
                let _ = DeleteObject(brush.into());
            }
            let inset = scale(4, dpi);
            let (l, t, rr, b) = (
                r.left + inset,
                r.top + inset,
                r.right - inset,
                r.bottom - inset,
            );
            let _ = MoveToEx(dc, l, t, None);
            let _ = LineTo(dc, rr, b);
            let _ = MoveToEx(dc, rr - 1, t, None);
            let _ = LineTo(dc, l - 1, b);
        }
        SelectObject(dc, old);
        let _ = DeleteObject(pen.into());
        ReleaseDC(Some(tab), dc);
    }
}

fn set_hover(tab: HWND, i: Option<usize>) {
    if HOVER.with(|h| h.replace(i)) != i {
        unsafe {
            let _ = InvalidateRect(Some(tab), None, false);
        }
    }
}

unsafe extern "system" fn subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    frame: usize,
) -> LRESULT {
    unsafe {
        match msg {
            WM_PAINT => {
                let r = DefSubclassProc(hwnd, msg, wparam, lparam);
                draw(hwnd);
                r
            }
            WM_MOUSEMOVE => {
                set_hover(hwnd, hit_close(hwnd, lparam));
                let mut tme = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                let _ = TrackMouseEvent(&mut tme);
                DefSubclassProc(hwnd, msg, wparam, lparam)
            }
            WM_MOUSELEAVE => {
                if PRESSED.with(|p| p.get()).is_none() {
                    set_hover(hwnd, None);
                }
                DefSubclassProc(hwnd, msg, wparam, lparam)
            }
            // 「×」の上のクリックはタブの選択にしない
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => match hit_close(hwnd, lparam) {
                Some(i) => {
                    PRESSED.with(|p| p.set(Some(i)));
                    SetCapture(hwnd);
                    let _ = InvalidateRect(Some(hwnd), None, false);
                    LRESULT(0)
                }
                None => DefSubclassProc(hwnd, msg, wparam, lparam),
            },
            WM_LBUTTONUP => match PRESSED.with(|p| p.take()) {
                Some(i) => {
                    let _ = ReleaseCapture();
                    if hit_close(hwnd, lparam) == Some(i) {
                        let _ = PostMessageW(
                            Some(HWND(frame as *mut _)),
                            WM_APP_CLOSE_TAB,
                            WPARAM(i),
                            LPARAM(0),
                        );
                    }
                    set_hover(hwnd, hit_close(hwnd, lparam));
                    let _ = InvalidateRect(Some(hwnd), None, false);
                    LRESULT(0)
                }
                None => DefSubclassProc(hwnd, msg, wparam, lparam),
            },
            // 中ボタンのクリックでも閉じる
            WM_MBUTTONUP => {
                if let Some(i) = hit_tab(hwnd, lparam) {
                    let _ = PostMessageW(
                        Some(HWND(frame as *mut _)),
                        WM_APP_CLOSE_TAB,
                        WPARAM(i),
                        LPARAM(0),
                    );
                }
                LRESULT(0)
            }
            // 右クリックのメニュー（フレームが出す）
            WM_RBUTTONUP => {
                if let Some(i) = hit_tab(hwnd, lparam) {
                    let _ = PostMessageW(
                        Some(HWND(frame as *mut _)),
                        WM_APP_TAB_MENU,
                        WPARAM(i),
                        LPARAM(0),
                    );
                }
                LRESULT(0)
            }
            WM_NCDESTROY => {
                let _ = RemoveWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID);
                DefSubclassProc(hwnd, msg, wparam, lparam)
            }
            _ => DefSubclassProc(hwnd, msg, wparam, lparam),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_targets() {
        assert_eq!(TabMenu::Close.targets(1, 4), vec![1]);
        assert_eq!(TabMenu::Others.targets(1, 4), vec![0, 2, 3]);
        assert_eq!(TabMenu::Right.targets(1, 4), vec![2, 3]);
        assert_eq!(TabMenu::Left.targets(1, 4), vec![0]);
        assert!(TabMenu::Right.targets(3, 4).is_empty());
        assert!(TabMenu::Left.targets(0, 4).is_empty());
        assert!(TabMenu::Others.targets(0, 1).is_empty());
        assert!(TabMenu::Close.targets(5, 4).is_empty());
    }
}
