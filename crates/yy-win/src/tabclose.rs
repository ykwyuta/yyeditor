//! タブの閉じるボタン。
//!
//! 標準のタブコントロールには閉じるボタンがないため、サブクラス化して各タブの右端に
//! 「×」を描き、そこのクリック（と中ボタンのクリック）でフレームにタブを閉じるよう知らせる。
//! 「×」の場所はタブの文字列の後ろに空白を足して空けておく（[`LABEL_PAD`]）。

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
            WM_NCDESTROY => {
                let _ = RemoveWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID);
                DefSubclassProc(hwnd, msg, wparam, lparam)
            }
            _ => DefSubclassProc(hwnd, msg, wparam, lparam),
        }
    }
}
