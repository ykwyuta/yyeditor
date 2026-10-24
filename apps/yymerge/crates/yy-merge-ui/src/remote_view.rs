use std::path::Path;
use windows::{
    Win32::{
        Foundation::*, Graphics::Gdi::*, System::LibraryLoader::*, UI::WindowsAndMessaging::*,
    },
    core::{HSTRING, w},
};
use yy_merge_core::remote::{HostKeyCheck, HostKeyQuestion, InputPrompter};

pub(super) fn download(
    path: &Path,
) -> yy_merge_core::Result<Option<yy_merge_core::remote::Download>> {
    if !path.to_str().is_some_and(|s| s.starts_with("ssh:")) {
        return Ok(None);
    }
    use std::sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc,
    };
    let cancel = Arc::new(yy_merge_core::Cancellation::new());
    struct Active(Arc<yy_merge_core::Cancellation>);
    impl Drop for Active {
        fn drop(&mut self) {
            self.0.cancel();
        }
    }
    let _active = Active(cancel.clone());
    let bytes = Arc::new(AtomicU64::new(0));
    let (tx, rx) = mpsc::channel();
    let source = path.to_owned();
    let stop = cancel.clone();
    let progress = bytes.clone();
    std::thread::spawn(move || {
        let result =
            yy_merge_core::remote::download_with_progress(&source, &Prompter, &stop, &|n| {
                progress.store(n, Ordering::Relaxed);
            });
        let _ = tx.send(result);
    });
    unsafe {
        let class = w!("YYMergeTransfer");
        let instance = GetModuleHandleW(None).map_err(|e| e.to_string())?;
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(proc),
            hInstance: instance.into(),
            lpszClassName: class,
            hbrBackground: GetSysColorBrush(COLOR_BTNFACE),
            ..Default::default()
        });
        let hwnd = CreateWindowExW(
            WS_EX_DLGMODALFRAME,
            class,
            w!("yymerge — SSH読み込み"),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            760,
            165,
            None,
            None,
            Some(instance.into()),
            None,
        )
        .map_err(|e| e.to_string())?;
        let mut dialog = DialogWindow {
            hwnd,
            state: Box::new(State {
                accepted: false,
                done: false,
            }),
            font: HFONT::default(),
        };
        let state = &mut dialog.state;
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, (&raw mut **state) as isize);
        let label = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("STATIC"),
            &HSTRING::from(path.display().to_string()),
            WS_CHILD | WS_VISIBLE,
            12,
            12,
            720,
            50,
            Some(hwnd),
            None,
            Some(instance.into()),
            None,
        )
        .map_err(|e| e.to_string())?;
        let _button = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("BUTTON"),
            w!("中止"),
            WS_CHILD | WS_VISIBLE,
            625,
            65,
            100,
            30,
            Some(hwnd),
            Some(HMENU(2usize as *mut _)),
            Some(instance.into()),
            None,
        )
        .map_err(|e| e.to_string())?;
        let _ = ShowWindow(hwnd, SW_SHOW);
        let mut previous = u64::MAX;
        let mut quit = None;
        let result = loop {
            match rx.try_recv() {
                Ok(result) => break result,
                Err(mpsc::TryRecvError::Disconnected) => break Err("SSH worker stopped".into()),
                Err(mpsc::TryRecvError::Empty) => {}
            }
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == WM_QUIT {
                    quit = Some(msg.wParam.0 as i32);
                    state.done = true;
                } else if msg.message == WM_KEYDOWN && msg.wParam.0 == 27 {
                    state.done = true;
                } else {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            if state.done {
                cancel.cancel();
            }
            let done = bytes.load(Ordering::Relaxed);
            if done != previous || state.done {
                let _ = SetWindowTextW(
                    label,
                    &HSTRING::from(format!(
                        "{}\n{} bytes{}",
                        path.display(),
                        done,
                        if state.done {
                            " — 中止しています"
                        } else {
                            ""
                        }
                    )),
                );
                previous = done;
            }
            let _ = MsgWaitForMultipleObjectsEx(None, 50, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
        };

        if let Some(code) = quit {
            PostQuitMessage(code);
        }
        result
    }
}

struct DialogWindow {
    hwnd: HWND,
    state: Box<State>,
    font: HFONT,
}
impl Drop for DialogWindow {
    fn drop(&mut self) {
        unsafe {
            SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
            let _ = DestroyWindow(self.hwnd);
            if !self.font.0.is_null() {
                let _ = DeleteObject(self.font.into());
            }
        }
    }
}
#[cfg(test)]
thread_local! {static TEST_REPLY: std::cell::RefCell<Option<(String,bool,bool)>>=const{std::cell::RefCell::new(None)};}

struct State {
    accepted: bool,
    done: bool,
}
unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let state = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State };
    if !state.is_null() && (msg == WM_CLOSE || msg == WM_COMMAND && matches!(wp.0 & 0xffff, 1 | 2))
    {
        unsafe {
            (*state).done = true;
            (*state).accepted = msg == WM_COMMAND && wp.0 & 0xffff == 1;
        }
        return LRESULT(0);
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

pub(super) fn ask(title: &str, initial: &str, secret: bool) -> Option<String> {
    unsafe {
        let class = w!("YYMergeInput");
        let instance = GetModuleHandleW(None).ok()?;
        let wc = WNDCLASSW {
            lpfnWndProc: Some(proc),
            hInstance: instance.into(),
            lpszClassName: class,
            hCursor: LoadCursorW(None, IDC_ARROW).ok()?,
            hbrBackground: GetSysColorBrush(COLOR_BTNFACE),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            WS_EX_DLGMODALFRAME,
            class,
            &HSTRING::from(title),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            760,
            175,
            None,
            None,
            Some(instance.into()),
            None,
        )
        .ok()?;
        let mut dialog = DialogWindow {
            hwnd,
            state: Box::new(State {
                accepted: false,
                done: false,
            }),
            font: HFONT::default(),
        };
        let state = &mut dialog.state;
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, (&raw mut **state) as isize);
        let edit = CreateWindowExW(
            WS_EX_CLIENTEDGE,
            w!("EDIT"),
            &HSTRING::from(initial),
            WS_CHILD
                | WS_VISIBLE
                | WS_TABSTOP
                | WINDOW_STYLE(ES_AUTOHSCROLL as u32 | if secret { ES_PASSWORD as u32 } else { 0 }),
            12,
            14,
            716,
            30,
            Some(hwnd),
            Some(HMENU(100usize as *mut _)),
            Some(instance.into()),
            None,
        )
        .ok()?;
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
            &HSTRING::from(yy_ui_win::font::BUNDLED_FAMILY),
        );
        dialog.font = font;
        yy_ui_win::font::register_gdi();
        SendMessageW(
            edit,
            WM_SETFONT,
            Some(WPARAM(font.0 as usize)),
            Some(LPARAM(1)),
        );
        for (id, label, x) in [(1, "OK", 520), (2, "キャンセル", 625)] {
            let button = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("BUTTON"),
                &HSTRING::from(label),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP,
                x,
                65,
                100,
                30,
                Some(hwnd),
                Some(HMENU(id as *mut _)),
                Some(instance.into()),
                None,
            )
            .ok()?;
            SendMessageW(
                button,
                WM_SETFONT,
                Some(WPARAM(font.0 as usize)),
                Some(LPARAM(1)),
            );
        }
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(Some(edit));
        #[cfg(test)]
        TEST_REPLY.with(|reply| {
            if let Some((value, secret, accepted)) = reply.borrow_mut().take() {
                assert_eq!(
                    GetWindowLongW(edit, GWL_STYLE) & ES_PASSWORD,
                    if secret { ES_PASSWORD } else { 0 }
                );
                let _ = SetWindowTextW(edit, &HSTRING::from(value));
                let _ = PostMessageW(
                    Some(hwnd),
                    WM_COMMAND,
                    WPARAM(if accepted { 1 } else { 2 }),
                    LPARAM(0),
                );
            }
        });
        let mut msg = MSG::default();
        let mut quit = None;
        while !state.done && GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if msg.message == WM_KEYDOWN && matches!(msg.wParam.0, 13 | 27) {
                state.accepted = msg.wParam.0 == 13;
                break;
            }
            if !IsDialogMessageW(hwnd, &msg).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        if msg.message == WM_QUIT {
            quit = Some(msg.wParam.0 as i32);
        }
        let mut text = vec![0u16; GetWindowTextLengthW(edit).max(0) as usize + 1];
        let length = GetWindowTextW(edit, &mut text);
        let result = state
            .accepted
            .then(|| String::from_utf16_lossy(&text[..length.max(0) as usize]));

        if let Some(code) = quit {
            PostQuitMessage(code);
        }
        result
    }
}

pub(super) struct Prompter;
impl InputPrompter for Prompter {
    fn keyboard_interactive(
        &self,
        host: &str,
        name: &str,
        instruction: &str,
        prompts: &[(String, bool)],
    ) -> Option<Vec<String>> {
        prompts
            .iter()
            .map(|(prompt, echo)| {
                ask(
                    &format!("{host} {name}: {instruction} — {prompt}"),
                    "",
                    !echo,
                )
            })
            .collect()
    }
    fn confirm_host_key(&self, q: &HostKeyQuestion) -> bool {
        let message = format!("{}:{}\n{}\n{}", q.host, q.port, q.algorithm, q.fingerprint);
        unsafe {
            if matches!(q.check, HostKeyCheck::Changed { .. }) {
                MessageBoxW(
                    None,
                    &HSTRING::from(format!(
                        "SSHホスト鍵が記録と異なるため接続を中止します。\n{message}"
                    )),
                    w!("yymerge SSH"),
                    MB_OK | MB_ICONERROR,
                );
                false
            } else {
                MessageBoxW(
                    None,
                    &HSTRING::from(format!(
                        "初めての接続先です。このホスト鍵を信頼しますか？\n{message}"
                    )),
                    w!("yymerge SSH"),
                    MB_YESNO | MB_ICONQUESTION,
                ) == IDYES
            }
        }
    }
    fn password(&self, label: &str) -> Option<String> {
        ask(&format!("SSH パスワード — {label}"), "", true)
    }
    fn passphrase(&self, key: &Path) -> Option<String> {
        ask(
            &format!("秘密鍵のパスフレーズ — {}", key.display()),
            "",
            true,
        )
    }
}

pub(super) fn open() -> yy_merge_core::Result<()> {
    let Some(left) = ask(
        "LEFT: ssh://[user@]host[:port]/絶対パス またはローカルパス",
        "ssh://",
        false,
    ) else {
        return Ok(());
    };
    let Some(right) = ask(
        "RIGHT: ssh://[user@]host[:port]/絶対パス またはローカルパス",
        "",
        false,
    ) else {
        return Ok(());
    };
    if left.is_empty() || right.is_empty() {
        return Err("左右のファイルを指定してください".into());
    }
    spawn(&yy_merge_cli::Gui {
        left: Some(left.into()),
        right: Some(right.into()),
        ..Default::default()
    })
}

pub(super) fn spawn(g: &yy_merge_cli::Gui) -> yy_merge_core::Result<()> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut command = std::process::Command::new(&exe);
    if exe
        .file_stem()
        .is_some_and(|s| s.eq_ignore_ascii_case("yymerge"))
    {
        command.arg("gui");
    }
    for (flag, value) in [("--left", &g.left), ("--right", &g.right)] {
        if let Some(path) = value {
            command.arg(flag).arg(path);
        }
    }
    if let Some(value) = &g.encoding {
        command.arg("--encoding").arg(value);
    }
    if let Some(value) = &g.inputs.left_encoding {
        command.arg("--left-encoding").arg(value);
    }
    if let Some(value) = &g.inputs.right_encoding {
        command.arg("--right-encoding").arg(value);
    }
    if g.inputs.ignore_encoding {
        command.arg("--ignore-encoding");
    }
    if g.large {
        command.arg("--large");
    }
    if let Some(label) = &g.left_label {
        command.arg("--left-label").arg(label);
    }
    if let Some(label) = &g.right_label {
        command.arg("--right-label").arg(label);
    }
    command.spawn().map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn credentials_are_masked_and_cancel_does_not_return_a_secret() {
        for (secret, accepted) in [(false, true), (true, true), (true, false)] {
            super::TEST_REPLY
                .with(|r| *r.borrow_mut() = Some(("日本語 test-value".into(), secret, accepted)));
            assert_eq!(
                super::ask("yymerge input regression", "", secret),
                accepted.then(|| "日本語 test-value".to_owned())
            );
        }
    }
}
