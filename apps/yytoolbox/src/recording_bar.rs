//! Small, visible recording controls excluded from the captured desktop.
use super::*;
use std::cell::Cell;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
const CLASS: windows::core::PCWSTR = w!("YYToolboxRecordingBar");
const STOP: usize = 1;
#[cfg(test)]
#[path = "recording_bar_tests.rs"]
mod tests;

pub(super) struct ControlBar(HWND);
struct State {
    owner: HWND,
    command: u16,
    status: HWND,
    hint: HWND,
    stop: HWND,
    font: Cell<HFONT>,
    background: HBRUSH,
    dpi: Cell<u32>,
    stopped: Cell<bool>,
    start: Instant,
    seconds: u32,
}
impl Drop for State {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(self.font.get().into());
            let _ = DeleteObject(self.background.into());
        }
    }
}
impl ControlBar {
    #[cfg(test)]
    pub(super) fn click_stop(&self) {
        unsafe {
            SendMessageW(data(self.0).unwrap().stop, BM_CLICK, None, None);
        }
    }
    pub(super) fn new(owner: HWND, seconds: u32, key: &str, command: u16) -> Result<Self> {
        unsafe {
            let instance = GetModuleHandleW(None).map_err(|e| e.to_string())?;
            let class = WNDCLASSW {
                hInstance: instance.into(),
                lpszClassName: CLASS,
                lpfnWndProc: Some(proc),
                hCursor: LoadCursorW(None, IDC_ARROW).map_err(|e| e.to_string())?,
                ..Default::default()
            };
            // A zero result is expected when another recording registered it.
            if RegisterClassW(&class) == 0 && GetLastError() != ERROR_CLASS_ALREADY_EXISTS {
                return Err(windows::core::Error::from_thread().to_string());
            }
            let dpi = GetDpiForWindow(owner).max(96);
            let scale = |n| n * dpi as i32 / 96;
            let mut monitor = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            if !GetMonitorInfoW(
                MonitorFromWindow(owner, MONITOR_DEFAULTTONEAREST),
                &mut monitor,
            )
            .as_bool()
            {
                return Err("録画操作バーの表示先を取得できません".into());
            }
            let width = scale(240).min(monitor.rcWork.right - monitor.rcWork.left);
            let height = scale(104).min(monitor.rcWork.bottom - monitor.rcWork.top);
            let hwnd = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
                CLASS,
                w!("yytoolbox 録画中"),
                WS_POPUP | WS_BORDER,
                monitor.rcWork.right - width - scale(12),
                monitor.rcWork.top + scale(12),
                width,
                height,
                Some(owner),
                None,
                Some(instance.into()),
                None,
            )
            .map_err(|e| e.to_string())?;
            let bar = Self(hwnd);
            // Never silently display controls in the recorded video.
            SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE)
                .map_err(|e| format!("操作バーを録画から除外できません: {e}"))?;
            let mut affinity = 0;
            GetWindowDisplayAffinity(hwnd, &mut affinity).map_err(|e| e.to_string())?;
            if affinity != WDA_EXCLUDEFROMCAPTURE.0 {
                return Err("録画操作バーのキャプチャ除外に対応していません".into());
            }
            let child = |class, title: &HSTRING, id| {
                CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    class,
                    title,
                    WS_CHILD
                        | WS_VISIBLE
                        | if id == STOP {
                            WS_TABSTOP | WINDOW_STYLE(BS_PUSHBUTTON as u32)
                        } else {
                            WINDOW_STYLE(0)
                        },
                    0,
                    0,
                    0,
                    0,
                    Some(hwnd),
                    Some(HMENU(id as *mut _)),
                    Some(instance.into()),
                    None,
                )
                .map_err(|e| e.to_string())
            };
            let status = child(w!("STATIC"), &HSTRING::from("● 録画中  00:00"), 2)?;
            let hint = child(
                w!("STATIC"),
                &HSTRING::from(if key == "なし（ボタンのみ）" {
                    "停止ボタンで保存".to_owned()
                } else {
                    format!("停止: {key} / ボタン")
                }),
                3,
            )?;
            let stop = child(w!("BUTTON"), &HSTRING::from("停止して保存"), STOP)?;
            let data = Box::new(State {
                owner,
                command,
                status,
                hint,
                stop,
                dpi: Cell::new(dpi),
                font: Cell::new(HFONT::default()),
                background: CreateSolidBrush(COLORREF(0x00282078)),
                stopped: Cell::new(false),
                start: Instant::now(),
                seconds,
            });
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(data) as isize);
            layout(hwnd, true);
            if SetTimer(Some(hwnd), 1, 250, None) == 0 {
                return Err("録画操作バーのタイマーを開始できません".into());
            }
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            let _ = UpdateWindow(hwnd);
            windows::Win32::Graphics::Dwm::DwmFlush().map_err(|e| e.to_string())?;
            Ok(bar)
        }
    }
    pub(super) fn stopping(&self) {
        unsafe {
            stop(self.0, false);
        }
    }
}
impl Drop for ControlBar {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.0);
        }
    }
}
unsafe fn data(hwnd: HWND) -> Option<&'static State> {
    unsafe { (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const State).as_ref() }
}
unsafe fn layout(hwnd: HWND, fonts: bool) {
    unsafe {
        let Some(s) = data(hwnd) else {
            return;
        };
        let d = |n| n * s.dpi.get() as i32 / 96;
        let mut rect = RECT::default();
        let _ = GetClientRect(hwnd, &mut rect);
        let width = (rect.right - d(20)).max(1);
        let _ = MoveWindow(s.status, d(10), d(7), width, d(21), true);
        let _ = MoveWindow(s.hint, d(10), d(30), width, d(20), true);
        let _ = MoveWindow(
            s.stop,
            d(10),
            d(56),
            width,
            (rect.bottom - d(64)).max(d(26)),
            true,
        );
        if fonts {
            let mut lf = LOGFONTW {
                lfHeight: -d(13),
                ..Default::default()
            };
            for (to, from) in lf.lfFaceName.iter_mut().zip("Segoe UI".encode_utf16()) {
                *to = from;
            }
            let font = CreateFontIndirectW(&lf);
            for child in [s.status, s.hint, s.stop] {
                SendMessageW(
                    child,
                    WM_SETFONT,
                    Some(WPARAM(font.0 as usize)),
                    Some(LPARAM(1)),
                );
            }
            let old = s.font.replace(font);
            if !old.is_invalid() {
                let _ = DeleteObject(old.into());
            }
        }
    }
}
unsafe fn stop(hwnd: HWND, notify: bool) {
    unsafe {
        if let Some(s) = data(hwnd)
            && !s.stopped.replace(true)
        {
            let _ = SetWindowTextW(s.status, w!("録画を保存しています…"));
            let _ = EnableWindow(s.stop, false);
            if notify {
                let _ = PostMessageW(
                    Some(s.owner),
                    WM_COMMAND,
                    WPARAM(usize::from(s.command)),
                    LPARAM(0),
                );
            }
        }
    }
}
unsafe extern "system" fn proc(hwnd: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        match message {
            WM_COMMAND if w.0 & 0xffff == STOP => {
                stop(hwnd, true);
                LRESULT(0)
            }
            WM_CLOSE => {
                stop(hwnd, true);
                LRESULT(0)
            }
            WM_TIMER => {
                if let Some(s) = data(hwnd)
                    && !s.stopped.get()
                {
                    let elapsed = s.start.elapsed().as_secs();
                    let title = HSTRING::from(format!(
                        "● 録画中  {:02}:{:02} / {:02}:{:02}",
                        elapsed / 60,
                        elapsed % 60,
                        s.seconds / 60,
                        s.seconds % 60
                    ));
                    let _ = SetWindowTextW(s.status, &title);
                }
                LRESULT(0)
            }
            WM_SIZE => {
                layout(hwnd, false);
                LRESULT(0)
            }
            WM_DPICHANGED => {
                if let Some(s) = data(hwnd) {
                    s.dpi.set((w.0 & 0xffff) as u32);
                }
                let rect = &*(l.0 as *const RECT);
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    rect.left,
                    rect.top,
                    rect.right - rect.left,
                    rect.bottom - rect.top,
                    SWP_NOACTIVATE | SWP_NOZORDER,
                );
                layout(hwnd, true);
                LRESULT(0)
            }
            WM_CTLCOLORSTATIC => {
                if let Some(s) = data(hwnd) {
                    let dc = HDC(w.0 as *mut _);
                    SetTextColor(dc, COLORREF(0xffffff));
                    SetBkColor(dc, COLORREF(0x00282078));
                    return LRESULT(s.background.0 as isize);
                }
                DefWindowProcW(hwnd, message, w, l)
            }
            WM_ERASEBKGND => {
                if let Some(s) = data(hwnd) {
                    let mut rect = RECT::default();
                    let _ = GetClientRect(hwnd, &mut rect);
                    FillRect(HDC(w.0 as *mut _), &rect, s.background);
                    return LRESULT(1);
                }
                DefWindowProcW(hwnd, message, w, l)
            }
            WM_NCHITTEST => {
                let result = DefWindowProcW(hwnd, message, w, l);
                if result.0 == HTCLIENT as isize {
                    LRESULT(HTCAPTION as isize)
                } else {
                    result
                }
            }
            WM_NCDESTROY => {
                let _ = KillTimer(Some(hwnd), 1);
                let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                if !ptr.is_null() {
                    drop(Box::from_raw(ptr));
                }
                DefWindowProcW(hwnd, message, w, l)
            }
            _ => DefWindowProcW(hwnd, message, w, l),
        }
    }
}
