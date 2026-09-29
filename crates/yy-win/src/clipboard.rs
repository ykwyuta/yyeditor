//! クリップボード（CF_UNICODETEXT）。

use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::CF_UNICODETEXT;

/// クリップボードを開いている間だけ保持し、閉じ忘れを防ぐ。
struct Opened;

impl Opened {
    fn open(owner: HWND) -> Option<Opened> {
        // 他のアプリが一時的に開いていることがあるため数回試す
        for _ in 0..5 {
            if unsafe { OpenClipboard(Some(owner)) }.is_ok() {
                return Some(Opened);
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        None
    }
}

impl Drop for Opened {
    fn drop(&mut self) {
        let _ = unsafe { CloseClipboard() };
    }
}

/// 矩形選択のデータであることを示す形式（Visual Studio・EmEditor などと共通。09 章 3.3）。
fn column_format() -> u32 {
    unsafe { RegisterClipboardFormatW(windows::core::w!("MSDEVColumnSelect")) }
}

/// クリップボードの文字列と、それが矩形選択のデータかを取り出す。
pub(crate) fn get_text(owner: HWND) -> Option<(String, bool)> {
    unsafe {
        IsClipboardFormatAvailable(CF_UNICODETEXT.0 as u32).ok()?;
        let column = IsClipboardFormatAvailable(column_format()).is_ok();
        let _open = Opened::open(owner)?;
        let handle = GetClipboardData(CF_UNICODETEXT.0 as u32).ok()?;
        let mem = HGLOBAL(handle.0);
        let ptr = GlobalLock(mem) as *const u16;
        if ptr.is_null() {
            return None;
        }
        let size = windows::Win32::System::Memory::GlobalSize(mem) / 2;
        let slice = std::slice::from_raw_parts(ptr, size);
        let len = slice.iter().position(|c| *c == 0).unwrap_or(slice.len());
        let text = String::from_utf16_lossy(&slice[..len]);
        let _ = GlobalUnlock(mem);
        Some((text, column))
    }
}

/// クリップボードに文字列を設定する。`column` なら矩形選択のデータとして印を付ける。
pub(crate) fn set_text(owner: HWND, text: &str, column: bool) -> bool {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let Some(_open) = Opened::open(owner) else {
            return false;
        };
        if EmptyClipboard().is_err() {
            return false;
        }
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
        if SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(mem.0))).is_err() {
            // 所有権が移らなかった場合は自分で解放する
            let _ = GlobalFree(Some(mem));
            return false;
        }
        if column
            && let Ok(m) = GlobalAlloc(GMEM_MOVEABLE, 1)
            && SetClipboardData(column_format(), Some(HANDLE(m.0))).is_err()
        {
            let _ = GlobalFree(Some(m));
        }
        true
    }
}
