#![cfg_attr(windows, windows_subsystem = "windows")]
#![allow(unsafe_code)]

#[cfg(windows)]
mod clipboard;
#[cfg(windows)]
mod history;
#[cfg(windows)]
mod templates;

#[cfg(windows)]
mod resident {
    use std::cell::RefCell;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::mpsc;
    use std::thread::JoinHandle;

    use windows::Win32::Foundation::{
        CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, POINT,
        RECT, WPARAM,
    };
    use windows::Win32::Graphics::Gdi::{
        CreateFontIndirectW, DeleteObject, GetMonitorInfoW, HFONT, HGDIOBJ, LOGFONTW,
        MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
    };
    use windows::Win32::System::DataExchange::{
        AddClipboardFormatListener, RemoveClipboardFormatListener,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Threading::{CreateMutexW, GetCurrentThreadId};
    use windows::Win32::UI::Controls::{
        EM_SETLIMITTEXT, ICC_TAB_CLASSES, INITCOMMONCONTROLSEX, InitCommonControlsEx, NMHDR,
        TCIF_TEXT, TCITEMW, TCM_ADJUSTRECT, TCM_GETCURSEL, TCM_INSERTITEMW, TCM_SETCURSEL,
        TCN_SELCHANGE, WC_TABCONTROLW,
    };
    use windows::Win32::UI::HiDpi::{
        GetDpiForMonitor, GetDpiForWindow, MDT_EFFECTIVE_DPI, SystemParametersInfoForDpi,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetKeyState, SetFocus, VK_CONTROL, VK_ESCAPE, VK_LCONTROL, VK_RCONTROL, VK_RETURN, VK_TAB,
    };
    use windows::Win32::UI::Shell::{
        NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW, Shell_NotifyIconW,
    };
    use windows::Win32::UI::WindowsAndMessaging::*;
    use windows::core::{Error, HRESULT, HSTRING, PCWSTR, Result, w};

    use crate::clipboard;
    use crate::history::{self, ControlSequence, Store};
    use crate::templates;

    const CLASS: windows::core::PCWSTR = w!("YYClipResidentWindow");
    const WM_TRAY: u32 = WM_APP + 1;
    const WM_SHOW_HISTORY: u32 = WM_APP + 2;
    const WM_HIDE_HISTORY: u32 = WM_APP + 3;
    const CAPTURE_TIMER: usize = 1;
    const CAPTURE_RETRIES: u8 = 10;
    const TRAY_ID: u32 = 1;
    const ID_TABS: usize = 101;
    const ID_LIST: usize = 102;
    const ID_EDIT: usize = 103;
    const ID_SAVE: usize = 104;
    const ID_CANCEL: usize = 105;
    const POPUP_WIDTH: i32 = 460;
    const POPUP_HEIGHT: i32 = 340;
    const EDITOR_WIDTH: i32 = 500;
    const EDITOR_HEIGHT: i32 = 330;
    static MENU_OPEN: AtomicBool = AtomicBool::new(false);
    static PICKER_OPEN: AtomicBool = AtomicBool::new(false);
    static MENU_EPOCH: AtomicU64 = AtomicU64::new(0);

    thread_local! {
        static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
        static HOOK_STATE: RefCell<Option<HookState>> = const { RefCell::new(None) };
    }

    struct HookState {
        target: HWND,
        keys: ControlSequence,
        epoch: u64,
    }

    struct State {
        listener: bool,
        hook_thread_id: Option<u32>,
        hook_thread: Option<JoinHandle<()>>,
        tray: bool,
        taskbar_message: u32,
        store: Store,
        templates: templates::Store,
        menu_open: bool,
        popup_open: bool,
        register_open: bool,
        message_open: bool,
        capture_attempts: u8,
        tabs: HWND,
        list: HWND,
        edit_label: HWND,
        edit: HWND,
        save_button: HWND,
        cancel_button: HWND,
        ui_font: HFONT,
        ui_dpi: u32,
        ui_scale: u32,
        font_px: i32,
        editing_path: Option<std::path::PathBuf>,
        previous_focus: HWND,
        history_entries: Vec<(std::path::PathBuf, String)>,
        template_entries: Vec<(std::path::PathBuf, String)>,
    }

    pub(super) fn run() -> Result<()> {
        unsafe {
            let mutex = CreateMutexW(None, false, w!("Local\\YYClipResident"))?;
            if GetLastError() == ERROR_ALREADY_EXISTS {
                let _ = CloseHandle(mutex);
                return Ok(());
            }

            let Some(dir) = history::default_dir() else {
                let _ = CloseHandle(mutex);
                return Err(Error::new(
                    HRESULT(0x80004005u32 as i32),
                    "APPDATA がありません",
                ));
            };
            let template_dir = templates::default_dir()
                .ok_or_else(|| Error::new(HRESULT(0x80004005u32 as i32), "APPDATA がありません"))?;
            let instance = GetModuleHandleW(None)?.into();
            if !InitCommonControlsEx(&INITCOMMONCONTROLSEX {
                dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
                dwICC: ICC_TAB_CLASSES,
            })
            .as_bool()
            {
                return Err(Error::from_thread());
            }
            let icon = LoadIconW(Some(instance), PCWSTR(std::ptr::without_provenance(1)))?;
            let class = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(window_proc),
                hInstance: instance,
                hIcon: icon,
                hIconSm: icon,
                lpszClassName: CLASS,
                ..Default::default()
            };
            if RegisterClassExW(&class) == 0 {
                let _ = CloseHandle(mutex);
                return Err(Error::from_thread());
            }
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                CLASS,
                w!("yyclip"),
                WS_POPUP | WS_BORDER,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(instance),
                None,
            )?;
            STATE.with(|cell| {
                *cell.borrow_mut() = Some(State {
                    listener: false,
                    hook_thread_id: None,
                    hook_thread: None,
                    tray: false,
                    taskbar_message: RegisterWindowMessageW(w!("TaskbarCreated")),
                    store: Store::new(dir),
                    templates: templates::Store::new(template_dir),
                    menu_open: false,
                    popup_open: false,
                    register_open: false,
                    message_open: false,
                    capture_attempts: 0,
                    tabs: HWND::default(),
                    list: HWND::default(),
                    edit_label: HWND::default(),
                    edit: HWND::default(),
                    save_button: HWND::default(),
                    cancel_button: HWND::default(),
                    ui_font: HFONT::default(),
                    ui_dpi: 0,
                    ui_scale: 1000,
                    font_px: 12,
                    editing_path: None,
                    previous_focus: HWND::default(),
                    history_entries: Vec::new(),
                    template_entries: Vec::new(),
                });
            });

            let setup = (|| {
                create_popup_controls(hwnd, instance)?;
                AddClipboardFormatListener(hwnd)?;
                STATE.with(|cell| cell.borrow_mut().as_mut().unwrap().listener = true);
                let (hook_thread_id, hook_thread) = start_hook(hwnd, instance)?;
                STATE.with(|cell| {
                    let mut state = cell.borrow_mut();
                    let state = state.as_mut().unwrap();
                    state.hook_thread_id = Some(hook_thread_id);
                    state.hook_thread = Some(hook_thread);
                });
                add_tray_icon(hwnd)?;
                STATE.with(|cell| cell.borrow_mut().as_mut().unwrap().tray = true);
                Ok::<(), Error>(())
            })();
            if let Err(error) = setup {
                let _ = DestroyWindow(hwnd);
                let _ = CloseHandle(mutex);
                return Err(error);
            }

            capture(hwnd);
            let mut message = MSG::default();
            while GetMessageW(&mut message, None, 0, 0).as_bool() {
                if popup_key(hwnd, &message) {
                    continue;
                }
                let view_open = STATE.with(|cell| {
                    cell.borrow()
                        .as_ref()
                        .is_some_and(|state| state.popup_open || state.register_open)
                });
                if view_open && IsDialogMessageW(hwnd, &message).as_bool() {
                    continue;
                }
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            let _ = CloseHandle(mutex);
        }
        Ok(())
    }

    fn tray_data(hwnd: HWND) -> NOTIFYICONDATAW {
        let mut data = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: TRAY_ID,
            ..Default::default()
        };
        let tip: Vec<u16> = "yyclip - クリップボード履歴".encode_utf16().collect();
        data.szTip[..tip.len()].copy_from_slice(&tip);
        data
    }

    unsafe fn add_tray_icon(hwnd: HWND) -> Result<()> {
        let mut data = tray_data(hwnd);
        data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        data.uCallbackMessage = WM_TRAY;
        let instance = unsafe { GetModuleHandleW(None)? }.into();
        data.hIcon = unsafe { LoadIconW(Some(instance), PCWSTR(std::ptr::without_provenance(1)))? };
        if unsafe { Shell_NotifyIconW(NIM_ADD, &data) }.as_bool() {
            Ok(())
        } else {
            Err(Error::from_thread())
        }
    }

    fn capture(hwnd: HWND) {
        let text = clipboard::get_text(history::MAX_BYTES);
        let saved = STATE.with(|cell| {
            let mut borrow = cell.borrow_mut();
            let state = borrow.as_mut().unwrap();
            text.is_some_and(|text| state.store.observe(text).is_ok())
        });
        if saved {
            unsafe {
                let _ = KillTimer(Some(hwnd), CAPTURE_TIMER);
            }
            STATE.with(|cell| cell.borrow_mut().as_mut().unwrap().capture_attempts = 0);
            let refresh = STATE.with(|cell| cell.borrow().as_ref().unwrap().popup_open);
            if refresh && active_tab() == 0 {
                refresh_list();
            }
        } else {
            let retry = STATE.with(|cell| {
                let mut borrow = cell.borrow_mut();
                let state = borrow.as_mut().unwrap();
                state.capture_attempts += 1;
                state.capture_attempts <= CAPTURE_RETRIES
            });
            if retry {
                unsafe {
                    let _ = SetTimer(Some(hwnd), CAPTURE_TIMER, 100, None);
                }
            } else {
                unsafe {
                    let _ = KillTimer(Some(hwnd), CAPTURE_TIMER);
                }
            }
        }
    }

    fn start_hook(hwnd: HWND, instance: HINSTANCE) -> Result<(u32, JoinHandle<()>)> {
        let target = hwnd.0 as usize;
        let module = instance.0 as usize;
        let (sender, receiver) = mpsc::sync_channel(1);
        let join = std::thread::spawn(move || unsafe {
            let hwnd = HWND(target as *mut _);
            let instance = HINSTANCE(module as *mut _);
            HOOK_STATE.with(|cell| {
                *cell.borrow_mut() = Some(HookState {
                    target: hwnd,
                    keys: ControlSequence::default(),
                    epoch: MENU_EPOCH.load(Ordering::SeqCst),
                });
            });
            let hook =
                match SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), Some(instance), 0) {
                    Ok(hook) => hook,
                    Err(error) => {
                        let _ = sender.send(Err(error));
                        return;
                    }
                };
            let mouse_hook =
                match SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), Some(instance), 0) {
                    Ok(hook) => hook,
                    Err(error) => {
                        let _ = UnhookWindowsHookEx(hook);
                        let _ = sender.send(Err(error));
                        return;
                    }
                };
            // PostThreadMessageW が届くよう、このスレッドにメッセージキューを作る。
            let mut message = MSG::default();
            let _ = PeekMessageW(&mut message, None, 0, 0, PM_NOREMOVE);
            let thread_id = GetCurrentThreadId();
            let _ = sender.send(Ok(thread_id));
            while GetMessageW(&mut message, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            let _ = UnhookWindowsHookEx(hook);
            let _ = UnhookWindowsHookEx(mouse_hook);
        });
        match receiver.recv() {
            Ok(Ok(id)) => Ok((id, join)),
            Ok(Err(error)) => {
                let _ = join.join();
                Err(error)
            }
            Err(_) => {
                let _ = join.join();
                Err(Error::new(
                    HRESULT(0x80004005u32 as i32),
                    "キーボード監視スレッドを開始できません",
                ))
            }
        }
    }

    /// 非表示の通知用ウィンドウをメニューの間だけ 1px 表示し、キー入力を受けられるようにする。
    unsafe fn show_menu_host(hwnd: HWND, point: POINT) {
        let _ = unsafe {
            SetWindowPos(
                hwnd,
                None,
                point.x,
                point.y,
                1,
                1,
                SWP_NOZORDER | SWP_NOACTIVATE | SWP_SHOWWINDOW,
            )
        };
        let _ = unsafe { SetForegroundWindow(hwnd) };
    }

    unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code == HC_ACTION as i32 {
            let message = wparam.0 as u32;
            let down = message == WM_KEYDOWN || message == WM_SYSKEYDOWN;
            let up = message == WM_KEYUP || message == WM_SYSKEYUP;
            if (down || up) && lparam.0 != 0 {
                let key = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
                if !key.flags.contains(LLKHF_INJECTED) {
                    let control_bit = match key.vkCode as u16 {
                        v if v == VK_LCONTROL.0 => 1,
                        v if v == VK_RCONTROL.0 => 2,
                        v if v == VK_CONTROL.0 => 4,
                        _ => 0,
                    };
                    HOOK_STATE.with(|cell| {
                        if let Ok(mut hook) = cell.try_borrow_mut()
                            && let Some(hook) = hook.as_mut()
                        {
                            let epoch = MENU_EPOCH.load(Ordering::SeqCst);
                            if hook.epoch != epoch {
                                hook.keys = ControlSequence::default();
                                hook.epoch = epoch;
                            }
                            if down
                                && key.vkCode as u16 == VK_ESCAPE.0
                                && PICKER_OPEN.load(Ordering::SeqCst)
                            {
                                let _ = unsafe {
                                    PostMessageW(
                                        Some(hook.target),
                                        WM_HIDE_HISTORY,
                                        WPARAM(0),
                                        LPARAM(0),
                                    )
                                };
                            }
                            if hook.keys.event(control_bit, down, key.time) {
                                let message = if PICKER_OPEN.load(Ordering::SeqCst) {
                                    WM_HIDE_HISTORY
                                } else if !MENU_OPEN.load(Ordering::SeqCst) {
                                    WM_SHOW_HISTORY
                                } else {
                                    0
                                };
                                if message != 0 {
                                    let _ = unsafe {
                                        PostMessageW(
                                            Some(hook.target),
                                            message,
                                            WPARAM(0),
                                            LPARAM(0),
                                        )
                                    };
                                }
                            }
                        }
                    });
                }
            }
        }
        unsafe { CallNextHookEx(None, code, wparam, lparam) }
    }

    unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code == HC_ACTION as i32
            && PICKER_OPEN.load(Ordering::SeqCst)
            && matches!(
                wparam.0 as u32,
                WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN
            )
            && lparam.0 != 0
        {
            let point = unsafe { (*(lparam.0 as *const MSLLHOOKSTRUCT)).pt };
            HOOK_STATE.with(|cell| {
                if let Some(hook) = cell.borrow().as_ref() {
                    let mut rect = RECT::default();
                    if unsafe { GetWindowRect(hook.target, &mut rect) }.is_ok()
                        && (point.x < rect.left
                            || point.x >= rect.right
                            || point.y < rect.top
                            || point.y >= rect.bottom)
                    {
                        let _ = unsafe {
                            PostMessageW(Some(hook.target), WM_HIDE_HISTORY, WPARAM(0), LPARAM(0))
                        };
                    }
                }
            });
        }
        unsafe { CallNextHookEx(None, code, wparam, lparam) }
    }

    fn begin_menu() -> Option<()> {
        STATE.with(|cell| {
            let mut borrow = cell.try_borrow_mut().ok()?;
            let state = borrow.as_mut()?;
            if state.menu_open {
                return None;
            }
            state.menu_open = true;
            MENU_OPEN.store(true, Ordering::SeqCst);
            MENU_EPOCH.fetch_add(1, Ordering::SeqCst);
            Some(())
        })
    }

    fn end_menu() {
        STATE.with(|cell| {
            if let Some(state) = cell.borrow_mut().as_mut() {
                state.menu_open = false;
            }
        });
        MENU_EPOCH.fetch_add(1, Ordering::SeqCst);
        MENU_OPEN.store(false, Ordering::SeqCst);
    }

    unsafe fn create_popup_controls(hwnd: HWND, instance: HINSTANCE) -> Result<()> {
        let tabs = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                WC_TABCONTROLW,
                PCWSTR::null(),
                WS_CHILD | WS_TABSTOP | WS_CLIPSIBLINGS,
                8,
                8,
                444,
                324,
                Some(hwnd),
                Some(HMENU(ID_TABS as *mut _)),
                Some(instance),
                None,
            )?
        };
        let list = unsafe {
            CreateWindowExW(
                WS_EX_CLIENTEDGE,
                w!("LISTBOX"),
                PCWSTR::null(),
                WS_CHILD
                    | WS_TABSTOP
                    | WS_VSCROLL
                    | WS_HSCROLL
                    | WS_CLIPSIBLINGS
                    | WINDOW_STYLE((LBS_NOTIFY | LBS_NOINTEGRALHEIGHT) as u32),
                18,
                44,
                424,
                280,
                Some(hwnd),
                Some(HMENU(ID_LIST as *mut _)),
                Some(instance),
                None,
            )?
        };
        let edit_label = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("定型文の内容（1 MiB まで）"),
                WS_CHILD,
                16,
                16,
                468,
                22,
                Some(hwnd),
                Some(HMENU(109usize as *mut _)),
                Some(instance),
                None,
            )?
        };
        let edit = unsafe {
            CreateWindowExW(
                WS_EX_CLIENTEDGE,
                w!("EDIT"),
                PCWSTR::null(),
                WS_CHILD
                    | WS_TABSTOP
                    | WS_VSCROLL
                    | WINDOW_STYLE((ES_MULTILINE | ES_AUTOVSCROLL | ES_WANTRETURN) as u32),
                16,
                46,
                468,
                220,
                Some(hwnd),
                Some(HMENU(ID_EDIT as *mut _)),
                Some(instance),
                None,
            )?
        };
        unsafe {
            SendMessageW(
                edit,
                EM_SETLIMITTEXT,
                Some(WPARAM(history::MAX_BYTES)),
                None,
            );
        }
        let mut buttons = Vec::new();
        for (id, title, x, width) in [
            (ID_SAVE, "保存", 286, 90),
            (ID_CANCEL, "キャンセル", 388, 96),
        ] {
            let button = unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    w!("BUTTON"),
                    &HSTRING::from(title),
                    WS_CHILD | WS_TABSTOP | WINDOW_STYLE(BS_PUSHBUTTON as u32),
                    x,
                    281,
                    width,
                    32,
                    Some(hwnd),
                    Some(HMENU(id as *mut _)),
                    Some(instance),
                    None,
                )?
            };
            buttons.push(button);
        }
        for (index, title) in ["クリップボード", "定型文"].iter().enumerate() {
            let mut wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
            let item = TCITEMW {
                mask: TCIF_TEXT,
                pszText: windows::core::PWSTR(wide.as_mut_ptr()),
                ..Default::default()
            };
            unsafe {
                SendMessageW(
                    tabs,
                    TCM_INSERTITEMW,
                    Some(WPARAM(index)),
                    Some(LPARAM((&item as *const TCITEMW) as isize)),
                );
            }
        }
        STATE.with(|cell| {
            let mut state = cell.borrow_mut();
            let state = state.as_mut().unwrap();
            state.tabs = tabs;
            state.list = list;
            state.edit_label = edit_label;
            state.edit = edit;
            state.save_button = buttons[0];
            state.cancel_button = buttons[1];
        });
        apply_ui_font(hwnd, unsafe { GetDpiForWindow(hwnd) }.max(96));
        Ok(())
    }

    /// yy-win::util::ui_font と同じ Windows メッセージフォントを使う。
    fn apply_ui_font(hwnd: HWND, dpi: u32) {
        let dpi = dpi.max(96);
        let unchanged = STATE.with(|cell| {
            cell.borrow()
                .as_ref()
                .is_some_and(|state| state.ui_dpi == dpi && !state.ui_font.0.is_null())
        });
        if unchanged {
            return;
        }
        let mut metrics = NONCLIENTMETRICSW {
            cbSize: std::mem::size_of::<NONCLIENTMETRICSW>() as u32,
            ..Default::default()
        };
        let log_font = if unsafe {
            SystemParametersInfoForDpi(
                SPI_GETNONCLIENTMETRICS.0,
                metrics.cbSize,
                Some(&mut metrics as *mut _ as *mut _),
                0,
                dpi,
            )
        }
        .is_ok()
        {
            metrics.lfMessageFont
        } else {
            let mut fallback = LOGFONTW {
                lfHeight: -(12 * dpi as i32 / 96),
                ..Default::default()
            };
            for (dest, code) in fallback
                .lfFaceName
                .iter_mut()
                .zip("Yu Gothic UI".encode_utf16())
            {
                *dest = code;
            }
            fallback
        };
        let font = unsafe { CreateFontIndirectW(&log_font) };
        if font.0.is_null() {
            return;
        }
        let font_px = log_font.lfHeight.unsigned_abs().max(1) as i32;
        let scale = layout_scale(dpi, font_px);
        let (old, controls) = STATE.with(|cell| {
            let mut borrow = cell.borrow_mut();
            let state = borrow.as_mut().unwrap();
            let old = std::mem::replace(&mut state.ui_font, font);
            state.ui_dpi = dpi;
            state.ui_scale = scale;
            state.font_px = font_px;
            let controls = [
                state.tabs,
                state.list,
                state.edit_label,
                state.edit,
                state.save_button,
                state.cancel_button,
            ];
            (old, controls)
        });
        unsafe {
            for control in controls {
                SendMessageW(
                    control,
                    WM_SETFONT,
                    Some(WPARAM(font.0 as usize)),
                    Some(LPARAM(1)),
                );
            }
            if !old.0.is_null() {
                let _ = DeleteObject(HGDIOBJ(old.0));
            }
        }
        layout_controls(hwnd);
    }

    fn layout_scale(dpi: u32, font_px: i32) -> u32 {
        (dpi.saturating_mul(1000) / 96)
            .max((font_px.max(1) as u32).saturating_mul(1000) / 12)
            .max(1000)
    }

    fn scaled(value: i32, scale: u32) -> i32 {
        ((value as i64 * scale as i64 + 500) / 1000) as i32
    }

    #[derive(Clone, Copy)]
    struct BoxRect {
        x: i32,
        y: i32,
        w: i32,
        h: i32,
    }

    struct EditorLayout {
        label: BoxRect,
        edit: BoxRect,
        save: BoxRect,
        cancel: BoxRect,
    }

    fn editor_layout(width: i32, height: i32, scale: u32, font_px: i32) -> EditorLayout {
        let pad = scaled(16, scale).max(font_px / 2 + 2);
        let gap = scaled(10, scale);
        let label_h = scaled(28, scale).max(font_px + scaled(10, scale));
        let button_h = scaled(36, scale).max(font_px + scaled(12, scale));
        let button_y = height - pad - button_h;
        let available = (width - 2 * pad - gap).max(2);
        let mut save_w = scaled(90, scale).max(font_px * 4);
        let mut cancel_w = scaled(96, scale).max(font_px * 7);
        if save_w + cancel_w > available {
            save_w = available / 2;
            cancel_w = available - save_w;
        }
        let edit_y = pad + label_h + gap;
        EditorLayout {
            label: BoxRect {
                x: pad,
                y: pad,
                w: (width - 2 * pad).max(1),
                h: label_h,
            },
            edit: BoxRect {
                x: pad,
                y: edit_y,
                w: (width - 2 * pad).max(1),
                h: (button_y - gap - edit_y).max(1),
            },
            save: BoxRect {
                x: width - pad - cancel_w - gap - save_w,
                y: button_y,
                w: save_w,
                h: button_h,
            },
            cancel: BoxRect {
                x: width - pad - cancel_w,
                y: button_y,
                w: cancel_w,
                h: button_h,
            },
        }
    }

    fn move_control(hwnd: HWND, rect: BoxRect) {
        unsafe {
            let _ = SetWindowPos(
                hwnd,
                None,
                rect.x,
                rect.y,
                rect.w,
                rect.h,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }

    fn layout_controls(hwnd: HWND) {
        let mut client = RECT::default();
        if unsafe { GetClientRect(hwnd, &mut client) }.is_err() {
            return;
        }
        let width = client.right - client.left;
        let height = client.bottom - client.top;
        if width <= 0 || height <= 0 {
            return;
        }
        let Some((tabs, list, label, edit, save, cancel, scale, font_px)) = STATE.with(|cell| {
            let borrow = cell.borrow();
            borrow.as_ref().map(|state| {
                (
                    state.tabs,
                    state.list,
                    state.edit_label,
                    state.edit,
                    state.save_button,
                    state.cancel_button,
                    state.ui_scale,
                    state.font_px,
                )
            })
        }) else {
            return;
        };
        if tabs.0.is_null() {
            return;
        }
        let pad = scaled(8, scale);
        let tab = BoxRect {
            x: pad,
            y: pad,
            w: (width - 2 * pad).max(1),
            h: (height - 2 * pad).max(1),
        };
        move_control(tabs, tab);
        let mut page = RECT {
            left: 0,
            top: 0,
            right: tab.w,
            bottom: tab.h,
        };
        unsafe {
            SendMessageW(
                tabs,
                TCM_ADJUSTRECT,
                Some(WPARAM(0)),
                Some(LPARAM((&mut page as *mut RECT) as isize)),
            );
        }
        let inset = scaled(2, scale);
        move_control(
            list,
            BoxRect {
                x: tab.x + page.left + inset,
                y: tab.y + page.top + inset,
                w: (page.right - page.left - 2 * inset).max(1),
                h: (page.bottom - page.top - 2 * inset).max(1),
            },
        );
        let editor = editor_layout(width, height, scale, font_px);
        move_control(label, editor.label);
        move_control(edit, editor.edit);
        move_control(save, editor.save);
        move_control(cancel, editor.cancel);
    }

    fn active_tab() -> usize {
        STATE.with(|cell| {
            let tabs = cell
                .borrow()
                .as_ref()
                .map_or(HWND::default(), |state| state.tabs);
            if tabs.0.is_null() {
                return 0;
            }
            let index = unsafe { SendMessageW(tabs, TCM_GETCURSEL, None, None).0 };
            if index == 1 { 1 } else { 0 }
        })
    }

    fn selected_index() -> Option<usize> {
        let list = STATE.with(|cell| cell.borrow().as_ref().map(|state| state.list))?;
        let index = unsafe { SendMessageW(list, LB_GETCURSEL, None, None).0 };
        (index >= 0).then_some(index as usize)
    }

    fn refresh_list() {
        let tab = active_tab();
        let (list, font_px, entries) = STATE.with(|cell| {
            let mut borrow = cell.borrow_mut();
            let state = borrow.as_mut().unwrap();
            if tab == 0 {
                state.history_entries = state.store.previews();
            } else {
                state.template_entries = state.templates.entries();
            }
            let entries = if tab == 0 {
                &state.history_entries
            } else {
                &state.template_entries
            };
            (
                state.list,
                state.font_px,
                entries
                    .iter()
                    .map(|(_, text)| history::label(text))
                    .collect::<Vec<_>>(),
            )
        });
        unsafe {
            SendMessageW(list, LB_RESETCONTENT, None, None);
            let mut widest = 0;
            for (index, label) in entries.iter().enumerate() {
                let row = HSTRING::from(format!("{}  {}", index + 1, label));
                widest = widest.max(row.len());
                SendMessageW(
                    list,
                    LB_ADDSTRING,
                    None,
                    Some(LPARAM(row.as_ptr() as isize)),
                );
            }
            if !entries.is_empty() {
                SendMessageW(list, LB_SETCURSEL, Some(WPARAM(0)), None);
            }
            SendMessageW(
                list,
                LB_SETHORIZONTALEXTENT,
                Some(WPARAM(widest.saturating_mul(font_px.max(1) as usize))),
                None,
            );
        }
    }

    fn show_history(hwnd: HWND) {
        if begin_menu().is_none() {
            return;
        }
        // 通知が遅れた場合でも、表示時点の最新テキストを一覧に反映する。
        capture(hwnd);
        let previous = unsafe { GetForegroundWindow() };
        STATE.with(|cell| {
            let mut state = cell.borrow_mut();
            let state = state.as_mut().unwrap();
            state.popup_open = true;
            state.previous_focus = previous;
        });
        PICKER_OPEN.store(true, Ordering::SeqCst);
        let (tabs, list, label, edit, save, cancel) = STATE.with(|cell| {
            let borrow = cell.borrow();
            let state = borrow.as_ref().unwrap();
            (
                state.tabs,
                state.list,
                state.edit_label,
                state.edit,
                state.save_button,
                state.cancel_button,
            )
        });
        unsafe {
            SendMessageW(tabs, TCM_SETCURSEL, Some(WPARAM(0)), None);
            for control in [label, edit, save, cancel] {
                let _ = ShowWindow(control, SW_HIDE);
            }
            for control in [tabs, list] {
                let _ = ShowWindow(control, SW_SHOW);
            }
            // タブのページ領域がリストを覆わないよう、リストを前面に置く。
            let _ = SetWindowPos(
                list,
                Some(HWND_TOP),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
        refresh_list();
        unsafe {
            show_at_cursor(hwnd, POPUP_WIDTH, POPUP_HEIGHT);
            let _ = SetForegroundWindow(hwnd);
            let _ = SetFocus(Some(list));
        }
    }

    unsafe fn show_at_cursor(hwnd: HWND, width: i32, height: i32) {
        let mut point = POINT::default();
        let _ = unsafe { GetCursorPos(&mut point) };
        unsafe {
            show_at_point(hwnd, point, width, height, None);
        }
    }

    unsafe fn show_at_point(
        hwnd: HWND,
        point: POINT,
        base_width: i32,
        base_height: i32,
        requested_dpi: Option<u32>,
    ) {
        let monitor = unsafe { MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST) };
        let mut x_dpi = 0;
        let mut y_dpi = 0;
        let monitor_dpi =
            unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut x_dpi, &mut y_dpi) }
                .ok()
                .map(|()| x_dpi)
                .filter(|dpi| *dpi >= 96);
        let dpi = requested_dpi
            .or(monitor_dpi)
            .unwrap_or_else(|| unsafe { GetDpiForWindow(hwnd) }.max(96));
        apply_ui_font(hwnd, dpi);
        let scale = STATE.with(|cell| cell.borrow().as_ref().map_or(1000, |state| state.ui_scale));
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let work = if unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
            info.rcWork
        } else {
            windows::Win32::Foundation::RECT {
                left: 0,
                top: 0,
                right: unsafe { GetSystemMetrics(SM_CXSCREEN) },
                bottom: unsafe { GetSystemMetrics(SM_CYSCREEN) },
            }
        };
        let width = scaled(base_width, scale).min(work.right - work.left).max(1);
        let height = scaled(base_height, scale)
            .min(work.bottom - work.top)
            .max(1);
        let x = point
            .x
            .clamp(work.left, (work.right - width).max(work.left));
        let y = point
            .y
            .clamp(work.top, (work.bottom - height).max(work.top));
        let _ = unsafe {
            SetWindowPos(
                hwnd,
                None,
                x,
                y,
                width,
                height,
                SWP_NOZORDER | SWP_SHOWWINDOW,
            )
        };
        layout_controls(hwnd);
    }

    fn show_register(hwnd: HWND, entry: Option<(std::path::PathBuf, String)>) {
        if begin_menu().is_none() {
            return;
        }
        let previous = unsafe { GetForegroundWindow() };
        let (tabs, list, label, edit, save, cancel) = STATE.with(|cell| {
            let mut borrow = cell.borrow_mut();
            let state = borrow.as_mut().unwrap();
            state.register_open = true;
            state.previous_focus = previous;
            state.editing_path = entry.as_ref().map(|(path, _)| path.clone());
            (
                state.tabs,
                state.list,
                state.edit_label,
                state.edit,
                state.save_button,
                state.cancel_button,
            )
        });
        unsafe {
            for control in [tabs, list] {
                let _ = ShowWindow(control, SW_HIDE);
            }
            for control in [label, edit, save, cancel] {
                let _ = ShowWindow(control, SW_SHOW);
            }
            let title = if entry.is_some() {
                w!("定型文を編集（1 MiB まで）")
            } else {
                w!("定型文を登録（1 MiB まで）")
            };
            let _ = SetWindowTextW(label, title);
            let value = entry.map_or_else(String::new, |(_, text)| text);
            let _ = SetWindowTextW(edit, &HSTRING::from(value));
            show_at_cursor(hwnd, EDITOR_WIDTH, EDITOR_HEIGHT);
            let _ = SetForegroundWindow(hwnd);
            let _ = SetFocus(Some(edit));
        }
    }

    fn close_popup(hwnd: HWND) {
        let previous = STATE.with(|cell| {
            let mut borrow = cell.borrow_mut();
            let state = borrow.as_mut()?;
            if !state.popup_open && !state.register_open {
                return None;
            }
            state.popup_open = false;
            state.register_open = false;
            state.editing_path = None;
            Some(state.previous_focus)
        });
        let Some(previous) = previous else {
            return;
        };
        PICKER_OPEN.store(false, Ordering::SeqCst);
        unsafe {
            let _ = ShowWindow(hwnd, SW_HIDE);
            if previous != hwnd && !previous.0.is_null() {
                let _ = SetForegroundWindow(previous);
            }
        }
        end_menu();
    }

    fn popup_key(hwnd: HWND, message: &MSG) -> bool {
        let (picker_open, register_open, list, tabs) = STATE.with(|cell| {
            let borrow = cell.borrow();
            let Some(state) = borrow.as_ref() else {
                return (false, false, HWND::default(), HWND::default());
            };
            (
                state.popup_open,
                state.register_open,
                state.list,
                state.tabs,
            )
        });
        if (!picker_open && !register_open) || message.message != WM_KEYDOWN {
            return false;
        }
        match message.wParam.0 as u16 {
            v if v == VK_ESCAPE.0 => {
                close_popup(hwnd);
                true
            }
            v if v == VK_RETURN.0
                && picker_open
                && (message.hwnd == list || message.hwnd == tabs) =>
            {
                select_item(hwnd);
                true
            }
            v if v == VK_TAB.0
                && picker_open
                && unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0 =>
            {
                let next = 1 - active_tab();
                unsafe {
                    SendMessageW(tabs, TCM_SETCURSEL, Some(WPARAM(next)), None);
                }
                refresh_list();
                true
            }
            _ => false,
        }
    }

    fn selected_text() -> Option<String> {
        let tab = active_tab();
        let index = selected_index()?;
        STATE.with(|cell| {
            let borrow = cell.borrow();
            let state = borrow.as_ref()?;
            if tab == 0 {
                state
                    .history_entries
                    .get(index)
                    .and_then(|(path, _)| state.store.load(path).ok())
            } else {
                state
                    .template_entries
                    .get(index)
                    .map(|(_, text)| text.clone())
            }
        })
    }

    fn select_item(hwnd: HWND) {
        let Some(text) = selected_text() else {
            return;
        };
        close_popup(hwnd);
        STATE.with(|cell| {
            if let Some(state) = cell.borrow_mut().as_mut() {
                state.store.suppress_next(text.clone());
            }
        });
        if !clipboard::set_text(hwnd, &text) {
            STATE.with(|cell| {
                if let Some(state) = cell.borrow_mut().as_mut() {
                    state.store.cancel_suppression();
                }
            });
        }
    }

    fn popup_message(hwnd: HWND, message: &str, flags: MESSAGEBOX_STYLE) -> i32 {
        STATE.with(|cell| cell.borrow_mut().as_mut().unwrap().message_open = true);
        let result =
            unsafe { MessageBoxW(Some(hwnd), &HSTRING::from(message), w!("yyclip"), flags) };
        STATE.with(|cell| cell.borrow_mut().as_mut().unwrap().message_open = false);
        result.0
    }

    fn save_template(hwnd: HWND) {
        let register_open = STATE.with(|cell| {
            cell.borrow()
                .as_ref()
                .is_some_and(|state| state.register_open)
        });
        if !register_open {
            return;
        }
        let (edit, editing_path) = STATE.with(|cell| {
            let borrow = cell.borrow();
            let state = borrow.as_ref().unwrap();
            (state.edit, state.editing_path.clone())
        });
        let length = unsafe { GetWindowTextLengthW(edit) };
        let mut buffer = vec![0u16; length as usize + 1];
        let copied = unsafe { GetWindowTextW(edit, &mut buffer) } as usize;
        let text = String::from_utf16_lossy(&buffer[..copied]);
        let result = STATE.with(|cell| {
            let borrow = cell.borrow();
            let store = &borrow.as_ref().unwrap().templates;
            if let Some(path) = editing_path {
                store.update(&path, &text).map(|()| path)
            } else {
                store.add(&text)
            }
        });
        match result {
            Ok(_) => close_popup(hwnd),
            Err(error) => {
                popup_message(hwnd, &error.to_string(), MB_ICONINFORMATION | MB_OK);
            }
        }
    }

    fn delete_item(hwnd: HWND, path: &std::path::Path) {
        if popup_message(
            hwnd,
            "選択した定型文を削除しますか？",
            MB_ICONQUESTION | MB_YESNO,
        ) != IDYES.0
        {
            return;
        }
        let result = STATE.with(|cell| cell.borrow().as_ref().unwrap().templates.remove(path));
        if let Err(error) = result {
            popup_message(hwnd, &error.to_string(), MB_ICONERROR | MB_OK);
        }
    }

    fn show_tray_menu(hwnd: HWND) {
        if begin_menu().is_none() {
            return;
        }
        let templates = STATE.with(|cell| cell.borrow().as_ref().unwrap().templates.entries());
        let mut selection = 0;
        unsafe {
            if let Ok(menu) = CreatePopupMenu() {
                let _ = AppendMenuW(menu, MF_STRING, 1, w!("履歴を表示"));
                let register_flags = if templates.len() >= templates::LIMIT {
                    MF_STRING | MF_GRAYED
                } else {
                    MF_STRING
                };
                let _ = AppendMenuW(menu, register_flags, 2, w!("定型文を登録..."));
                if templates.is_empty() {
                    let _ = AppendMenuW(menu, MF_STRING | MF_GRAYED, 4, w!("定型文を編集"));
                    let _ = AppendMenuW(menu, MF_STRING | MF_GRAYED, 5, w!("定型文を削除"));
                } else if let (Ok(edit_menu), Ok(delete_menu)) =
                    (CreatePopupMenu(), CreatePopupMenu())
                {
                    for (index, (_, body)) in templates.iter().enumerate() {
                        let label = HSTRING::from(history::label(body).replace('&', "&&"));
                        let _ = AppendMenuW(edit_menu, MF_STRING, 100 + index, &label);
                        let _ = AppendMenuW(delete_menu, MF_STRING, 200 + index, &label);
                    }
                    let _ = AppendMenuW(menu, MF_POPUP, edit_menu.0 as usize, w!("定型文を編集"));
                    let _ = AppendMenuW(menu, MF_POPUP, delete_menu.0 as usize, w!("定型文を削除"));
                }
                let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
                let _ = AppendMenuW(menu, MF_STRING, 3, w!("終了"));
                let mut point = POINT::default();
                let _ = GetCursorPos(&mut point);
                let previous = GetForegroundWindow();
                show_menu_host(hwnd, point);
                selection = TrackPopupMenuEx(
                    menu,
                    TPM_RETURNCMD.0 | TPM_RIGHTBUTTON.0,
                    point.x,
                    point.y,
                    hwnd,
                    None,
                )
                .0;
                let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
                let _ = DestroyMenu(menu);
                let _ = ShowWindow(hwnd, SW_HIDE);
                if previous != hwnd && !previous.0.is_null() {
                    let _ = SetForegroundWindow(previous);
                }
            }
        }
        end_menu();
        match selection {
            1 => show_history(hwnd),
            2 => show_register(hwnd, None),
            3 => {
                let _ = unsafe { DestroyWindow(hwnd) };
            }
            100..=119 => {
                if let Some(entry) = templates.get(selection as usize - 100) {
                    show_register(hwnd, Some(entry.clone()));
                }
            }
            200..=219 => {
                if let Some((path, _)) = templates.get(selection as usize - 200) {
                    delete_item(hwnd, path);
                }
            }
            _ => {}
        }
    }

    extern "system" fn window_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        let taskbar_message = STATE.with(|cell| {
            cell.borrow()
                .as_ref()
                .map_or(0, |state| state.taskbar_message)
        });
        if taskbar_message != 0 && msg == taskbar_message {
            let _ = unsafe { add_tray_icon(hwnd) };
            return LRESULT(0);
        }
        match msg {
            WM_CLIPBOARDUPDATE => {
                STATE.with(|cell| cell.borrow_mut().as_mut().unwrap().capture_attempts = 0);
                capture(hwnd);
                LRESULT(0)
            }
            WM_TIMER if wparam.0 == CAPTURE_TIMER => {
                capture(hwnd);
                LRESULT(0)
            }
            WM_SHOW_HISTORY => {
                show_history(hwnd);
                LRESULT(0)
            }
            WM_HIDE_HISTORY => {
                close_popup(hwnd);
                LRESULT(0)
            }
            WM_TRAY => {
                match lparam.0 as u32 {
                    WM_LBUTTONUP | WM_LBUTTONDBLCLK => show_history(hwnd),
                    WM_RBUTTONUP => show_tray_menu(hwnd),
                    _ => {}
                }
                LRESULT(0)
            }
            WM_NOTIFY => {
                if lparam.0 != 0 {
                    let header = unsafe { &*(lparam.0 as *const NMHDR) };
                    let tabs = STATE.with(|cell| cell.borrow().as_ref().map(|state| state.tabs));
                    if tabs == Some(header.hwndFrom) && header.code == TCN_SELCHANGE {
                        refresh_list();
                    }
                }
                LRESULT(0)
            }
            WM_COMMAND => {
                let id = wparam.0 & 0xffff;
                let notification = (wparam.0 >> 16) & 0xffff;
                match id {
                    ID_SAVE => save_template(hwnd),
                    ID_CANCEL => close_popup(hwnd),
                    ID_LIST if notification == LBN_DBLCLK as usize => select_item(hwnd),
                    _ => {}
                }
                LRESULT(0)
            }
            WM_ACTIVATE => {
                let close = STATE.with(|cell| {
                    cell.borrow().as_ref().is_some_and(|state| {
                        state.popup_open && !state.message_open && (wparam.0 & 0xffff) == 0
                    })
                });
                if close {
                    close_popup(hwnd);
                }
                LRESULT(0)
            }
            WM_SIZE => {
                layout_controls(hwnd);
                LRESULT(0)
            }
            WM_DPICHANGED => {
                let dpi = (wparam.0 & 0xffff) as u32;
                let view = STATE.with(|cell| {
                    cell.borrow()
                        .as_ref()
                        .map(|state| (state.popup_open, state.register_open))
                });
                if let Some((picker, register)) = view {
                    if (picker || register) && lparam.0 != 0 {
                        let suggested = unsafe { &*(lparam.0 as *const RECT) };
                        let point = POINT {
                            x: suggested.left,
                            y: suggested.top,
                        };
                        let (width, height) = if register {
                            (EDITOR_WIDTH, EDITOR_HEIGHT)
                        } else {
                            (POPUP_WIDTH, POPUP_HEIGHT)
                        };
                        unsafe {
                            show_at_point(hwnd, point, width, height, Some(dpi));
                        }
                    } else {
                        apply_ui_font(hwnd, dpi);
                    }
                }
                LRESULT(0)
            }
            WM_SETTINGCHANGE => {
                let view = STATE.with(|cell| {
                    let mut borrow = cell.borrow_mut();
                    let state = borrow.as_mut()?;
                    state.ui_dpi = 0;
                    Some((state.popup_open, state.register_open))
                });
                if let Some((picker, register)) = view {
                    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
                    if picker || register {
                        let mut rect = RECT::default();
                        if unsafe { GetWindowRect(hwnd, &mut rect) }.is_ok() {
                            let point = POINT {
                                x: rect.left,
                                y: rect.top,
                            };
                            let (width, height) = if register {
                                (EDITOR_WIDTH, EDITOR_HEIGHT)
                            } else {
                                (POPUP_WIDTH, POPUP_HEIGHT)
                            };
                            unsafe {
                                show_at_point(hwnd, point, width, height, Some(dpi));
                            }
                        }
                    } else {
                        apply_ui_font(hwnd, dpi);
                    }
                }
                LRESULT(0)
            }
            WM_CLOSE => {
                close_popup(hwnd);
                LRESULT(0)
            }
            WM_DESTROY => {
                MENU_OPEN.store(false, Ordering::SeqCst);
                PICKER_OPEN.store(false, Ordering::SeqCst);
                unsafe {
                    let _ = KillTimer(Some(hwnd), CAPTURE_TIMER);
                }
                STATE.with(|cell| {
                    if let Some(state) = cell.borrow_mut().take() {
                        unsafe {
                            if !state.ui_font.0.is_null() {
                                let _ = DeleteObject(HGDIOBJ(state.ui_font.0));
                            }
                            if state.tray {
                                let data = tray_data(hwnd);
                                let _ = Shell_NotifyIconW(NIM_DELETE, &data);
                            }
                            if let Some(id) = state.hook_thread_id {
                                let _ = PostThreadMessageW(id, WM_QUIT, WPARAM(0), LPARAM(0));
                            }
                            if state.listener {
                                let _ = RemoveClipboardFormatListener(hwnd);
                            }
                        }
                        if let Some(join) = state.hook_thread {
                            let _ = join.join();
                        }
                    }
                });
                unsafe { PostQuitMessage(0) };
                LRESULT(0)
            }
            _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
        }
    }

    #[cfg(test)]
    mod layout_tests {
        use super::*;

        #[test]
        fn editor_controls_fit_scaled_fonts_and_work_areas() {
            for (dpi, font_px, work_width, work_height) in [
                (96, 12, 1920, 1080),
                (144, 19, 1920, 1080),
                (192, 24, 1600, 900),
                (96, 28, 1280, 720),
                (192, 24, 800, 600),
                (96, 28, 520, 346),
            ] {
                let scale = layout_scale(dpi, font_px);
                let width = scaled(EDITOR_WIDTH, scale).min(work_width);
                let height = scaled(EDITOR_HEIGHT, scale).min(work_height);
                let layout = editor_layout(width, height, scale, font_px);
                assert!(layout.label.h > font_px);
                assert!(layout.edit.y >= layout.label.y + layout.label.h);
                assert!(layout.edit.y + layout.edit.h < layout.save.y);
                assert!(layout.save.y + layout.save.h <= height);
                assert!(layout.cancel.y + layout.cancel.h <= height);
                assert!(layout.save.x >= 0 && layout.cancel.x >= layout.save.x + layout.save.w);
                assert!(layout.cancel.x + layout.cancel.w <= width);
            }
        }
    }
}

#[cfg(windows)]
fn main() {
    if let Err(error) = resident::run() {
        unsafe {
            let message = windows::core::HSTRING::from(format!("yyclip を起動できません: {error}"));
            let _ = windows::Win32::UI::WindowsAndMessaging::MessageBoxW(
                None,
                &message,
                windows::core::w!("yyclip"),
                windows::Win32::UI::WindowsAndMessaging::MB_ICONERROR
                    | windows::Win32::UI::WindowsAndMessaging::MB_OK,
            );
        }
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("yyclip は Windows 専用です");
}
