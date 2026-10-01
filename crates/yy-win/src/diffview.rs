//! Virtual side-by-side comparison window.

use std::fs::File;
use std::io::BufReader;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver};

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateFontW, CreateSolidBrush,
    DEFAULT_CHARSET, DeleteObject, ETO_CLIPPED, EndPaint, ExtTextOutW, FF_MODERN, FW_NORMAL,
    FillRect, InvalidateRect, OUT_DEFAULT_PRECIS, PAINTSTRUCT, SelectObject, SetBkMode,
    SetTextColor, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::SetScrollInfo;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PCWSTR};
use yy_buffer::Snapshot;

use crate::DIFF_CLASS;
use crate::diffstream::{self, Index, Kind, NONE, Row};

const HEADER: i32 = 40;
const LINE_HEIGHT: i32 = 20;
const SCALE: i32 = 1_000_000;
const READY: u32 = WM_APP + 73;
const TIMER: usize = 7;

struct DiffWindow {
    left: Snapshot,
    right: Snapshot,
    left_name: String,
    right_name: String,
    font: windows::Win32::Graphics::Gdi::HFONT,
    top: u64,
    scroll_x: u64,
    index: Option<Index>,
    reader: Option<BufReader<File>>,
    receiver: Receiver<std::io::Result<Option<Index>>>,
    cancel: Arc<AtomicBool>,
    progress: Arc<AtomicU64>,
    error: Option<String>,
}

impl Drop for DiffWindow {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        // Windows cannot unlink an open index file. Close the reader first.
        self.reader.take();
        self.index.take();
    }
}

pub(crate) fn show(
    owner: HWND,
    left_name: &str,
    left: &Snapshot,
    right_name: &str,
    right: &Snapshot,
) -> Result<(), String> {
    let (tx, rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let progress = Arc::new(AtomicU64::new(0));
    unsafe {
        let instance = GetModuleHandleW(None).map_err(|e| e.to_string())?;
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            DIFF_CLASS,
            &HSTRING::from(format!("{left_name} ⇔ {right_name} - yyeditor 比較")),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE | WS_VSCROLL | WS_HSCROLL,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1200,
            750,
            Some(owner),
            None,
            Some(instance.into()),
            None,
        )
        .map_err(|e| e.to_string())?;
        crate::font::register_gdi();
        let font = CreateFontW(
            -16,
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
            FF_MODERN.0 as u32,
            &HSTRING::from(crate::font::BUNDLED_FAMILY),
        );
        let state = Box::new(DiffWindow {
            left: left.clone(),
            right: right.clone(),
            left_name: left_name.into(),
            right_name: right_name.into(),
            font,
            top: 0,
            scroll_x: 0,
            index: None,
            reader: None,
            receiver: rx,
            cancel: cancel.clone(),
            progress: progress.clone(),
            error: None,
        });
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(state) as isize);
        let _ = SetTimer(Some(hwnd), TIMER, 250, None);
        let l = left.clone();
        let r = right.clone();
        let handle = hwnd.0 as usize;
        std::thread::spawn(move || {
            let result = diffstream::compare(&l, &r, &cancel, &progress);
            if let Err(e) = tx.send(result) {
                drop(e.0);
            } else {
                let _ = PostMessageW(Some(HWND(handle as *mut _)), READY, WPARAM(0), LPARAM(0));
            }
        });
        let _ = ShowWindow(hwnd, SW_SHOW);
    }
    Ok(())
}

unsafe fn state_ptr(hwnd: HWND) -> *mut DiffWindow {
    unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut DiffWindow }
}

fn page_rows(hwnd: HWND) -> u64 {
    let mut rc = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut rc);
    }
    ((rc.bottom - HEADER).max(0) / LINE_HEIGHT).max(1) as u64
}

fn update_scroll(hwnd: HWND, state: &DiffWindow) {
    let rows = state.index.as_ref().map_or(0, |i| i.rows);
    let max_top = rows.saturating_sub(page_rows(hwnd));
    let pos = if max_top == 0 {
        0
    } else {
        (state.top as u128 * SCALE as u128 / max_top as u128) as i32
    };
    let si = SCROLLINFO {
        cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
        fMask: SIF_ALL | SIF_DISABLENOSCROLL,
        nMin: 0,
        nMax: SCALE,
        nPage: if max_top == 0 { SCALE as u32 + 1 } else { 1 },
        nPos: pos,
        nTrackPos: 0,
    };
    unsafe {
        SetScrollInfo(hwnd, SB_VERT, &si, true);
    }
    let max_x = state.index.as_ref().map_or(0, |i| i.max_line_bytes);
    let x_pos = if max_x == 0 {
        0
    } else {
        (state.scroll_x as u128 * SCALE as u128 / max_x as u128) as i32
    };
    let hsi = SCROLLINFO {
        cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
        fMask: SIF_ALL | SIF_DISABLENOSCROLL,
        nMin: 0,
        nMax: SCALE,
        nPage: if max_x == 0 { SCALE as u32 + 1 } else { 1 },
        nPos: x_pos,
        nTrackPos: 0,
    };
    unsafe {
        SetScrollInfo(hwnd, SB_HORZ, &hsi, true);
    }
}

fn scroll_to(hwnd: HWND, state: &mut DiffWindow, top: u64) {
    let rows = state.index.as_ref().map_or(0, |i| i.rows);
    state.top = top.min(rows.saturating_sub(page_rows(hwnd)));
    update_scroll(hwnd, state);
    unsafe {
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
}

fn scroll_x_to(hwnd: HWND, state: &mut DiffWindow, x: u64) {
    state.scroll_x = x.min(state.index.as_ref().map_or(0, |i| i.max_line_bytes));
    update_scroll(hwnd, state);
    unsafe {
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
}

fn text(hdc: windows::Win32::Graphics::Gdi::HDC, s: &str, x: i32, y: i32, clip: &RECT) {
    let units: Vec<u16> = s.encode_utf16().collect();
    unsafe {
        let _ = ExtTextOutW(
            hdc,
            x,
            y,
            ETO_CLIPPED,
            Some(clip),
            PCWSTR(units.as_ptr()),
            units.len() as u32,
            None,
        );
    }
}

fn visible_line(snapshot: &Snapshot, offset: u64, len: u64, skip: u64) -> String {
    if offset == NONE || skip >= len {
        return String::new();
    }
    // Never materialize an entire line: a single line may itself be gigabytes long.
    let start = offset + skip;
    let bytes = snapshot.read(start..start + (len - skip).min(4096));
    let stop = bytes.len();
    let mut first = 0;
    if skip > 0 {
        while first < stop && bytes[first] & 0xc0 == 0x80 {
            first += 1;
        }
    }
    String::from_utf8_lossy(&bytes[first..stop])
        .chars()
        .fold(String::new(), |mut s, c| {
            match c {
                '\0'..='\x1f' if c != '\t' => {
                    s.push(char::from_u32(0x2400 + c as u32).unwrap_or(' '))
                }
                '\x7f' => s.push('\u{2421}'),
                '\u{85}' => s.push('\u{2424}'),
                // 字形のない C1 制御文字（本文の表示と同じ）
                '\u{80}'..='\u{9f}' => s.push_str(&format!("<{:02X}>", c as u32)),
                _ => s.push(c),
            }
            s
        })
}

fn paint(hwnd: HWND, state: &mut DiffWindow) {
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut ps);
        let mut rc = RECT::default();
        let _ = GetClientRect(hwnd, &mut rc);
        let white = CreateSolidBrush(COLORREF(0x00ffffff));
        let gray = CreateSolidBrush(COLORREF(0x00ececec));
        let red = CreateSolidBrush(COLORREF(0x00e9e9ff));
        let green = CreateSolidBrush(COLORREF(0x00e9ffe9));
        let _ = FillRect(hdc, &rc, white);
        let _ = FillRect(
            hdc,
            &RECT {
                bottom: HEADER,
                ..rc
            },
            gray,
        );
        let old = SelectObject(hdc, state.font.into());
        let _ = SetBkMode(hdc, TRANSPARENT);
        let _ = SetTextColor(hdc, COLORREF(0x00202020));
        let mid = rc.right / 2;
        let lc = RECT {
            left: 0,
            top: 0,
            right: mid,
            bottom: rc.bottom,
        };
        let rc2 = RECT {
            left: mid,
            top: 0,
            right: rc.right,
            bottom: rc.bottom,
        };
        text(hdc, &format!("左: {}", state.left_name), 8, 4, &lc);
        text(hdc, &format!("右: {}", state.right_name), mid + 8, 4, &rc2);
        let status = if let Some(ref error) = state.error {
            format!("比較エラー: {error}")
        } else if let Some(ref index) = state.index {
            format!("差分 {} 行 / 全 {} 行", index.changes, index.rows)
        } else {
            let total = state.left.len().saturating_add(state.right.len()).max(1);
            let done = state.progress.load(Ordering::Relaxed).min(total);
            format!("比較中… {}%", done.saturating_mul(100) / total)
        };
        text(hdc, &status, 8, 22, &rc);
        if let (Some(index), Some(reader)) = (&state.index, &mut state.reader) {
            match index.read_rows(reader, state.top, page_rows(hwnd) as usize + 1) {
                Ok(rows) => {
                    for (i, row) in rows.iter().enumerate() {
                        let y = HEADER + i as i32 * LINE_HEIGHT;
                        if y >= rc.bottom {
                            break;
                        }
                        if row.kind != Kind::Equal {
                            if row.left != NONE {
                                let _ = FillRect(
                                    hdc,
                                    &RECT {
                                        top: y,
                                        bottom: y + LINE_HEIGHT,
                                        ..lc
                                    },
                                    red,
                                );
                            }
                            if row.right != NONE {
                                let _ = FillRect(
                                    hdc,
                                    &RECT {
                                        top: y,
                                        bottom: y + LINE_HEIGHT,
                                        ..rc2
                                    },
                                    green,
                                );
                            }
                        }
                        if row.left != NONE {
                            text(
                                hdc,
                                &row_text(row, true, &state.left, state.scroll_x),
                                8,
                                y + 1,
                                &lc,
                            );
                        }
                        if row.right != NONE {
                            text(
                                hdc,
                                &row_text(row, false, &state.right, state.scroll_x),
                                mid + 8,
                                y + 1,
                                &rc2,
                            );
                        }
                    }
                }
                Err(e) => state.error = Some(e.to_string()),
            }
        }
        let _ = SelectObject(hdc, old);
        for brush in [white, gray, red, green] {
            let _ = DeleteObject(brush.into());
        }
        let _ = EndPaint(hwnd, &ps);
    }
}

fn row_text(row: &Row, left: bool, snapshot: &Snapshot, scroll_x: u64) -> String {
    let (offset, number, len) = if left {
        (row.left, row.left_line, row.left_len)
    } else {
        (row.right, row.right_line, row.right_len)
    };
    let mark = if row.kind == Kind::Equal {
        ' '
    } else if left {
        '-'
    } else {
        '+'
    };
    format!(
        "{:>8} {mark} {}",
        number + 1,
        visible_line(snapshot, offset, len, scroll_x)
    )
}

pub(crate) extern "system" fn proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_PAINT => unsafe {
            let p = state_ptr(hwnd);
            if p.is_null() {
                DefWindowProcW(hwnd, msg, wparam, lparam)
            } else {
                paint(hwnd, &mut *p);
                LRESULT(0)
            }
        },
        READY => unsafe {
            let p = state_ptr(hwnd);
            if !p.is_null() {
                if let Ok(result) = (*p).receiver.try_recv() {
                    match result {
                        Ok(Some(index)) => match index.open() {
                            Ok(reader) => {
                                (*p).reader = Some(reader);
                                (*p).index = Some(index);
                            }
                            Err(e) => (*p).error = Some(e.to_string()),
                        },
                        Ok(None) => {}
                        Err(e) => (*p).error = Some(e.to_string()),
                    }
                }
                let _ = KillTimer(Some(hwnd), TIMER);
                update_scroll(hwnd, &*p);
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            LRESULT(0)
        },
        WM_TIMER if wparam.0 == TIMER => {
            unsafe {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            LRESULT(0)
        }
        WM_SIZE => unsafe {
            let p = state_ptr(hwnd);
            if !p.is_null() {
                scroll_to(hwnd, &mut *p, (*p).top);
            }
            LRESULT(0)
        },
        WM_VSCROLL => unsafe {
            let p = state_ptr(hwnd);
            if !p.is_null() {
                let page = page_rows(hwnd);
                let top = match SCROLLBAR_COMMAND((wparam.0 & 0xffff) as i32) {
                    SB_LINEUP => (*p).top.saturating_sub(1),
                    SB_LINEDOWN => (*p).top.saturating_add(1),
                    SB_PAGEUP => (*p).top.saturating_sub(page),
                    SB_PAGEDOWN => (*p).top.saturating_add(page),
                    SB_TOP => 0,
                    SB_BOTTOM => u64::MAX,
                    SB_THUMBTRACK | SB_THUMBPOSITION => {
                        let mut si = SCROLLINFO {
                            cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
                            fMask: SIF_TRACKPOS,
                            ..Default::default()
                        };
                        let _ = GetScrollInfo(hwnd, SB_VERT, &mut si);
                        let max_top = (*p)
                            .index
                            .as_ref()
                            .map_or(0, |i| i.rows.saturating_sub(page));
                        (si.nTrackPos.max(0) as u128 * max_top as u128 / SCALE as u128) as u64
                    }
                    _ => (*p).top,
                };
                scroll_to(hwnd, &mut *p, top);
            }
            LRESULT(0)
        },
        WM_HSCROLL => unsafe {
            let p = state_ptr(hwnd);
            if !p.is_null() {
                let x = match SCROLLBAR_COMMAND((wparam.0 & 0xffff) as i32) {
                    SB_LINELEFT => (*p).scroll_x.saturating_sub(16),
                    SB_LINERIGHT => (*p).scroll_x.saturating_add(16),
                    SB_PAGELEFT => (*p).scroll_x.saturating_sub(512),
                    SB_PAGERIGHT => (*p).scroll_x.saturating_add(512),
                    SB_LEFT => 0,
                    SB_RIGHT => u64::MAX,
                    SB_THUMBTRACK | SB_THUMBPOSITION => {
                        let mut si = SCROLLINFO {
                            cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
                            fMask: SIF_TRACKPOS,
                            ..Default::default()
                        };
                        let _ = GetScrollInfo(hwnd, SB_HORZ, &mut si);
                        let max_x = (*p).index.as_ref().map_or(0, |i| i.max_line_bytes);
                        (si.nTrackPos.max(0) as u128 * max_x as u128 / SCALE as u128) as u64
                    }
                    _ => (*p).scroll_x,
                };
                scroll_x_to(hwnd, &mut *p, x);
            }
            LRESULT(0)
        },
        WM_MOUSEWHEEL => unsafe {
            let p = state_ptr(hwnd);
            if !p.is_null() {
                let delta = ((wparam.0 >> 16) as u16) as i16;
                let top = if delta > 0 {
                    (*p).top.saturating_sub(3)
                } else {
                    (*p).top.saturating_add(3)
                };
                scroll_to(hwnd, &mut *p, top);
            }
            LRESULT(0)
        },
        WM_MOUSEHWHEEL => unsafe {
            let p = state_ptr(hwnd);
            if !p.is_null() {
                let delta = ((wparam.0 >> 16) as u16) as i16;
                let x = if delta > 0 {
                    (*p).scroll_x.saturating_add(48)
                } else {
                    (*p).scroll_x.saturating_sub(48)
                };
                scroll_x_to(hwnd, &mut *p, x);
            }
            LRESULT(0)
        },
        WM_KEYDOWN => unsafe {
            let p = state_ptr(hwnd);
            if p.is_null() {
                return DefWindowProcW(hwnd, msg, wparam, lparam);
            }
            let page = page_rows(hwnd);
            if wparam.0 == 0x25 || wparam.0 == 0x27 {
                let x = if wparam.0 == 0x25 {
                    (*p).scroll_x.saturating_sub(16)
                } else {
                    (*p).scroll_x.saturating_add(16)
                };
                scroll_x_to(hwnd, &mut *p, x);
                return LRESULT(0);
            }
            let top = match wparam.0 as u32 {
                0x26 => (*p).top.saturating_sub(1),
                0x28 => (*p).top.saturating_add(1),
                0x21 => (*p).top.saturating_sub(page),
                0x22 => (*p).top.saturating_add(page),
                0x24 => 0,
                0x23 => u64::MAX,
                _ => return DefWindowProcW(hwnd, msg, wparam, lparam),
            };
            scroll_to(hwnd, &mut *p, top);
            LRESULT(0)
        },
        WM_NCDESTROY => unsafe {
            let p = SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) as *mut DiffWindow;
            if !p.is_null() {
                let state = Box::from_raw(p);
                state.cancel.store(true, Ordering::Relaxed);
                let _ = KillTimer(Some(hwnd), TIMER);
                let _ = DeleteObject(state.font.into());
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        },
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn window_renders_virtual_rows_and_removes_index_on_close() {
        unsafe {
            let instance = GetModuleHandleW(None).unwrap();
            let class = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(proc),
                hInstance: instance.into(),
                lpszClassName: DIFF_CLASS,
                ..Default::default()
            };
            assert_ne!(RegisterClassExW(&class), 0);
            let left = Snapshot::from_bytes(b"a\nold\nc\n");
            let right = Snapshot::from_bytes(b"a\nnew\nc\n");
            show(HWND::default(), "left", &left, "right", &right).unwrap();
            let hwnd = FindWindowW(DIFF_CLASS, None).unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            while (*state_ptr(hwnd)).index.is_none() && Instant::now() < deadline {
                let mut msg = MSG::default();
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            assert_eq!((*state_ptr(hwnd)).index.as_ref().unwrap().changes, 1);
            let path = (*state_ptr(hwnd)).index.as_ref().unwrap().path.clone();
            let _ = SendMessageW(hwnd, WM_VSCROLL, Some(WPARAM(SB_BOTTOM.0 as usize)), None);
            let _ = SendMessageW(hwnd, WM_HSCROLL, Some(WPARAM(SB_RIGHT.0 as usize)), None);
            assert_eq!((*state_ptr(hwnd)).scroll_x, 3);
            assert!(DestroyWindow(hwnd).is_ok());
            assert!(!path.exists());
        }
    }

    #[test]
    fn horizontal_window_stays_within_its_line() {
        let snap = Snapshot::from_bytes("日本語abc\nnext".as_bytes());
        assert_eq!(visible_line(&snap, 0, 12, 9), "abc");
        assert_eq!(visible_line(&snap, 0, 12, 12), "");
        assert_eq!(visible_line(&snap, 13, 4, 0), "next");
    }
}
