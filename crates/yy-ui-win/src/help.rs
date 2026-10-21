//! Embedded Markdown help using the yy preview renderer and an offline text fallback.
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use webview2_com::{
    AcceleratorKeyPressedEventHandler, CreateCoreWebView2ControllerCompletedHandler,
    CreateCoreWebView2EnvironmentCompletedHandler, Microsoft::Web::WebView2::Win32::*,
    NavigationCompletedEventHandler, NavigationStartingEventHandler,
    NewWindowRequestedEventHandler, WebResourceRequestedEventHandler,
};
use windows::{
    Win32::{
        Foundation::*,
        Graphics::Gdi::*,
        System::{
            Com::{CoTaskMemFree, IStream},
            LibraryLoader::GetModuleHandleW,
        },
        UI::{
            Controls::{EM_GETFIRSTVISIBLELINE, EM_LINESCROLL},
            HiDpi::*,
            Input::KeyboardAndMouse::*,
            Shell::{SHCreateMemStream, ShellExecuteW},
            WindowsAndMessaging::*,
        },
    },
    core::{HSTRING, Interface, PCWSTR, PWSTR, Result, w},
};

const CLASS: PCWSTR = w!("YYSeriesHelp");
const FAILED: u32 = WM_APP + 70;
const READY: u32 = WM_APP + 71;
struct State {
    hwnd: HWND,
    edit: HWND,
    font: HFONT,
    html: String,
    line: Cell<usize>,
    controller: RefCell<Option<ICoreWebView2Controller>>,
    web: RefCell<Option<ICoreWebView2>>,
}
thread_local! { static WINDOW: RefCell<Option<Rc<State>>> = const { RefCell::new(None) }; }

/// Keep one modeless help window. Section anchors use the yy Markdown `{#id}` syntax.
pub fn show(markdown: &str, title: &str, section: Option<&str>, cache_name: &str) -> Result<()> {
    let line = section
        .and_then(|s| yy_preview::help::section_line(markdown, s))
        .unwrap_or(0);
    if let Some(s) = WINDOW.with(|w| w.borrow().clone()) {
        s.line.set(line);
        scroll(&s);
        unsafe {
            let _ = ShowWindow(s.hwnd, SW_RESTORE);
            let _ = SetForegroundWindow(s.hwnd);
        }
        return Ok(());
    }
    unsafe {
        let instance = GetModuleHandleW(None)?;
        RegisterClassW(&WNDCLASSW {
            hInstance: instance.into(),
            lpszClassName: CLASS,
            lpfnWndProc: Some(proc),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            hbrBackground: GetSysColorBrush(COLOR_WINDOW),
            ..Default::default()
        });
        let dpi = GetDpiForSystem().max(96) as i32;
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            CLASS,
            &HSTRING::from(title),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            860 * dpi / 96,
            760 * dpi / 96,
            None,
            None,
            Some(instance.into()),
            None,
        )?;
        let edit = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("EDIT"),
            &HSTRING::from(yy_preview::help::plain_text(markdown).replace('\n', "\r\n")),
            WS_CHILD
                | WS_VISIBLE
                | WS_VSCROLL
                | WS_HSCROLL
                | WINDOW_STYLE(
                    (ES_MULTILINE | ES_READONLY | ES_AUTOVSCROLL | ES_AUTOHSCROLL) as u32,
                ),
            0,
            0,
            1,
            1,
            Some(hwnd),
            Some(HMENU(std::ptr::without_provenance_mut(1))),
            None,
            None,
        )?;
        let mut metrics = NONCLIENTMETRICSW {
            cbSize: std::mem::size_of::<NONCLIENTMETRICSW>() as u32,
            ..Default::default()
        };
        let _ = SystemParametersInfoForDpi(
            SPI_GETNONCLIENTMETRICS.0,
            metrics.cbSize,
            Some((&mut metrics as *mut NONCLIENTMETRICSW).cast()),
            0,
            dpi as u32,
        );
        if metrics.lfMessageFont.lfHeight == 0 {
            metrics.lfMessageFont.lfHeight = -12 * dpi / 96;
        }
        let font = CreateFontIndirectW(&metrics.lfMessageFont);
        SendMessageW(
            edit,
            WM_SETFONT,
            Some(WPARAM(font.0 as usize)),
            Some(LPARAM(1)),
        );
        let registry = yy_syntax::Registry::builtin();
        let body = yy_preview::markdown_to_html(markdown, Some(&registry));
        let opts = yy_preview::PageOptions {
            has_folder: false,
            token_colors: yy_config::Colors::default()
                .syntax
                .iter()
                .map(|(k, c)| (k.clone(), format!("#{:02X}{:02X}{:02X}", c.r, c.g, c.b)))
                .collect(),
        };
        let s = Rc::new(State {
            hwnd,
            edit,
            font,
            html: yy_preview::markdown_page(&body, &opts),
            line: Cell::new(line),
            controller: RefCell::new(None),
            web: RefCell::new(None),
        });
        WINDOW.with(|w| *w.borrow_mut() = Some(s.clone()));
        layout(&s);
        scroll(&s);
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        // The text remains usable while WebView2 starts or if startup fails.
        if std::env::var_os("YY_HELP_TEXT_ONLY").is_none() && start(&s, cache_name).is_err() {
            let _ = PostMessageW(Some(hwnd), FAILED, WPARAM(0), LPARAM(0));
        }
        Ok(())
    }
}
pub fn contains(hwnd: HWND) -> bool {
    WINDOW.with(|w| {
        w.borrow()
            .as_ref()
            .is_some_and(|s| unsafe { hwnd == s.hwnd || IsChild(s.hwnd, hwnd).as_bool() })
    })
}
pub fn close() {
    if let Some(s) = WINDOW.with(|w| w.borrow().clone()) {
        unsafe {
            let _ = DestroyWindow(s.hwnd);
        }
    }
}
/// Route text-fallback keys separately from the application's SQL/editor shortcuts.
pub fn translate(msg: &MSG) -> bool {
    if !contains(msg.hwnd) {
        return false;
    }
    if msg.message == WM_KEYDOWN
        && (msg.wParam.0 == VK_ESCAPE.0 as usize
            || (msg.wParam.0 == b'W' as usize
                && unsafe { GetAsyncKeyState(VK_CONTROL.0 as i32) < 0 }))
    {
        close();
        return true;
    }
    unsafe {
        let _ = TranslateMessage(msg);
        DispatchMessageW(msg);
    }
    true
}
fn scroll(s: &State) {
    unsafe {
        let first = SendMessageW(s.edit, EM_GETFIRSTVISIBLELINE, None, None).0;
        SendMessageW(
            s.edit,
            EM_LINESCROLL,
            Some(WPARAM(0)),
            Some(LPARAM(s.line.get() as isize - first)),
        );
        if let Some(web) = s.web.borrow().clone() {
            let _ = web.PostWebMessageAsJson(&HSTRING::from(yy_preview::update_message(
                None,
                Some(s.line.get()),
            )));
        }
    }
}
fn layout(s: &State) {
    unsafe {
        let mut r = RECT::default();
        let _ = GetClientRect(s.hwnd, &mut r);
        let _ = MoveWindow(s.edit, 0, 0, r.right, r.bottom, true);
        if let Some(c) = s.controller.borrow().clone() {
            let _ = c.SetBounds(r);
        }
    }
}
unsafe extern "system" fn proc(h: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    let state = WINDOW.with(|s| s.borrow().clone());
    if let Some(s) = state.filter(|s| s.hwnd == h) {
        match msg {
            WM_SIZE => {
                layout(&s);
                return LRESULT(0);
            }
            READY => {
                layout(&s);
                scroll(&s);
                let controller = s.controller.borrow().clone();
                unsafe {
                    if let Some(c) = controller {
                        let _ = c.SetIsVisible(true);
                    }
                    let _ = ShowWindow(s.edit, SW_HIDE);
                }
                return LRESULT(0);
            }
            FAILED => {
                let controller = s.controller.borrow().clone();
                unsafe {
                    if let Some(c) = controller {
                        let _ = c.SetIsVisible(false);
                    }
                    let _ = ShowWindow(s.edit, SW_SHOW);
                }
                scroll(&s);
                return LRESULT(0);
            }
            WM_DESTROY => {
                WINDOW.with(|w| w.borrow_mut().take());
                let controller = s.controller.borrow_mut().take();
                if let Some(c) = controller {
                    unsafe {
                        let _ = c.Close();
                    }
                }
                unsafe {
                    let _ = DeleteObject(s.font.into());
                }
                return LRESULT(0);
            }
            _ => {}
        }
    }
    unsafe { DefWindowProcW(h, msg, w, l) }
}

// Same static loader as yyeditor: no WebView2Loader.dll needs to be distributed.
#[cfg(target_env = "msvc")]
#[link(name = "WebView2LoaderStatic", kind = "static")]
unsafe extern "system" {
    fn CreateCoreWebView2EnvironmentWithOptions(
        browser: PCWSTR,
        data: PCWSTR,
        options: *mut std::ffi::c_void,
        handler: *mut std::ffi::c_void,
    ) -> windows::core::HRESULT;
}
fn start(s: &Rc<State>, cache_name: &str) -> Result<()> {
    let weak = Rc::downgrade(s);
    let handler = CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(
        move |result, env: Option<ICoreWebView2Environment>| {
            let Some(s) = weak.upgrade() else {
                return Ok(());
            };
            let Some(env) = env.filter(|_| result.is_ok()) else {
                unsafe {
                    let _ = PostMessageW(Some(s.hwnd), FAILED, WPARAM(0), LPARAM(0));
                }
                return Ok(());
            };
            let weak = Rc::downgrade(&s);
            let env2 = env.clone();
            let handler = CreateCoreWebView2ControllerCompletedHandler::create(Box::new(
                move |result, controller| {
                    let Some(s) = weak.upgrade() else {
                        if let Some(c) = controller {
                            unsafe {
                                let _ = c.Close();
                            }
                        }
                        return Ok(());
                    };
                    if controller
                        .filter(|_| result.is_ok())
                        .is_some_and(|c| setup(&s, env2.clone(), c).is_ok())
                    {
                        return Ok(());
                    }
                    unsafe {
                        let _ = PostMessageW(Some(s.hwnd), FAILED, WPARAM(0), LPARAM(0));
                    }
                    Ok(())
                },
            ));
            if unsafe { env.CreateCoreWebView2Controller(s.hwnd, &handler) }.is_err() {
                unsafe {
                    let _ = PostMessageW(Some(s.hwnd), FAILED, WPARAM(0), LPARAM(0));
                }
            }
            Ok(())
        },
    ));
    let folder = std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(cache_name)
        .join("HelpWebView2");
    std::fs::create_dir_all(&folder).map_err(|_| windows::core::Error::from(E_FAIL))?;
    let data = HSTRING::from(folder.as_os_str());
    #[cfg(target_env = "msvc")]
    unsafe {
        CreateCoreWebView2EnvironmentWithOptions(
            PCWSTR::null(),
            PCWSTR(data.as_ptr()),
            std::ptr::null_mut(),
            handler.as_raw(),
        )
        .ok()
    }
    #[cfg(not(target_env = "msvc"))]
    {
        let _ = (data, handler);
        Err(windows::core::Error::from(E_NOTIMPL))
    }
}
fn setup(
    s: &Rc<State>,
    env: ICoreWebView2Environment,
    controller: ICoreWebView2Controller,
) -> Result<()> {
    unsafe {
        let web = controller.CoreWebView2()?;
        let settings = web.Settings()?;
        settings.SetIsStatusBarEnabled(false)?;
        settings.SetAreDevToolsEnabled(false)?;
        web.AddWebResourceRequestedFilter(
            w!("https://yy-preview.local/*"),
            COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
        )?;
        let mut token = 0;
        let weak = Rc::downgrade(s);
        web.add_WebResourceRequested(
            &WebResourceRequestedEventHandler::create(Box::new(move |_, args| {
                let (Some(s), Some(args)) = (weak.upgrade(), args) else {
                    return Ok(());
                };
                let mut uri = PWSTR::null();
                args.Request()?.Uri(&mut uri)?;
                let url = uri.to_string().unwrap_or_default();
                CoTaskMemFree(Some(uri.as_ptr().cast()));
                let path = url
                    .strip_prefix("https://yy-preview.local")
                    .unwrap_or("")
                    .split(['?', '#'])
                    .next()
                    .unwrap_or("");
                let data = if path == "/index.html" {
                    Some((s.html.as_bytes().to_vec(), "text/html; charset=utf-8"))
                } else {
                    yy_preview::asset(path).map(|(b, m)| (b.into_owned(), m))
                };
                let (body, mime) = data.unwrap_or_else(|| (b"not found".to_vec(), "text/plain"));
                let stream: Option<IStream> = SHCreateMemStream(Some(&body));
                let response = env.CreateWebResourceResponse(
                    stream.as_ref(),
                    200,
                    w!("OK"),
                    &HSTRING::from(format!("Content-Type: {mime}\r\nCache-Control: no-store")),
                )?;
                args.SetResponse(&response)
            })),
            &mut token,
        )?;
        let weak = Rc::downgrade(s);
        web.add_NavigationCompleted(
            &NavigationCompletedEventHandler::create(Box::new(move |_, args| {
                let Some(s) = weak.upgrade() else {
                    return Ok(());
                };
                let mut success = windows::core::BOOL(0);
                if let Some(args) = args {
                    args.IsSuccess(&mut success)?;
                }
                let _ = PostMessageW(
                    Some(s.hwnd),
                    if success.as_bool() { READY } else { FAILED },
                    WPARAM(0),
                    LPARAM(0),
                );
                Ok(())
            })),
            &mut token,
        )?;
        web.add_NavigationStarting(
            &NavigationStartingEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else {
                    return Ok(());
                };
                let mut uri = PWSTR::null();
                args.Uri(&mut uri)?;
                let url = uri.to_string().unwrap_or_default();
                CoTaskMemFree(Some(uri.as_ptr().cast()));
                if url != yy_preview::PAGE_URL
                    && !url.starts_with("https://yy-preview.local/index.html#")
                {
                    args.SetCancel(true)?;
                    let mut user = windows::core::BOOL(0);
                    args.IsUserInitiated(&mut user)?;
                    if user.as_bool() && (url.starts_with("https://") || url.starts_with("http://"))
                    {
                        ShellExecuteW(
                            None,
                            w!("open"),
                            &HSTRING::from(url),
                            None,
                            None,
                            SW_SHOWNORMAL,
                        );
                    }
                }
                Ok(())
            })),
            &mut token,
        )?;
        web.add_NewWindowRequested(
            &NewWindowRequestedEventHandler::create(Box::new(move |_, args| {
                if let Some(args) = args {
                    args.SetHandled(true)?;
                }
                Ok(())
            })),
            &mut token,
        )?;
        let hwnd = s.hwnd;
        controller.add_AcceleratorKeyPressed(
            &AcceleratorKeyPressedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else {
                    return Ok(());
                };
                let mut kind = COREWEBVIEW2_KEY_EVENT_KIND::default();
                args.KeyEventKind(&mut kind)?;
                if kind == COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN {
                    let mut key = 0;
                    args.VirtualKey(&mut key)?;
                    if key == VK_ESCAPE.0 as u32
                        || (key == b'W' as u32 && GetAsyncKeyState(VK_CONTROL.0 as i32) < 0)
                    {
                        args.SetHandled(true)?;
                        let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
                    }
                }
                Ok(())
            })),
            &mut token,
        )?;
        *s.controller.borrow_mut() = Some(controller.clone());
        *s.web.borrow_mut() = Some(web.clone());
        layout(s);
        controller.SetIsVisible(false)?;
        web.Navigate(w!("https://yy-preview.local/index.html"))?;
        Ok(())
    }
}
