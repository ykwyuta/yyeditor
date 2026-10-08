//! Windows の Unicode テキストクリップボード。

use std::time::Duration;

use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    SetClipboardData,
};
use windows::Win32::System::Memory::{
    GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
};
use windows::Win32::System::Ole::CF_UNICODETEXT;

struct Opened;

impl Opened {
    fn open(owner: Option<HWND>) -> Option<Self> {
        for _ in 0..5 {
            if unsafe { OpenClipboard(owner) }.is_ok() {
                return Some(Self);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        None
    }
}

impl Drop for Opened {
    fn drop(&mut self) {
        let _ = unsafe { CloseClipboard() };
    }
}

/// サイズ上限を超える本文は全体を確保せずに除外する。
pub(crate) fn get_text(max_bytes: usize) -> Option<String> {
    unsafe {
        let _opened = Opened::open(None)?;
        IsClipboardFormatAvailable(CF_UNICODETEXT.0 as u32).ok()?;
        let handle = GetClipboardData(CF_UNICODETEXT.0 as u32).ok()?;
        let mem = HGLOBAL(handle.0);
        let units = GlobalSize(mem) / 2;
        if units == 0 {
            return None;
        }
        let ptr = GlobalLock(mem) as *const u16;
        if ptr.is_null() {
            return None;
        }
        let chars = std::slice::from_raw_parts(ptr, units.min(max_bytes.saturating_add(1)));
        let end = chars.iter().position(|&c| c == 0);
        let text = end.map(|n| String::from_utf16_lossy(&chars[..n]));
        let _ = GlobalUnlock(mem);
        text.filter(|s| !s.is_empty() && s.len() <= max_bytes)
    }
}

pub(crate) fn set_text(owner: HWND, text: &str) -> bool {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let Ok(mem) = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2) else {
            return false;
        };
        let ptr = GlobalLock(mem) as *mut u16;
        if ptr.is_null() {
            let _ = GlobalFree(Some(mem));
            return false;
        }
        std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
        let _ = GlobalUnlock(mem);
        let Some(_opened) = Opened::open(Some(owner)) else {
            let _ = GlobalFree(Some(mem));
            return false;
        };
        if EmptyClipboard().is_err() {
            let _ = GlobalFree(Some(mem));
            return false;
        }
        if SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(mem.0))).is_err() {
            let _ = GlobalFree(Some(mem));
            return false;
        }
        true
    }
}
