//! Per-input encoding settings; no App borrow is held during the modal loop.
use super::*;
use yy_merge_cli::InputOptions;
const CLASS: PCWSTR = w!("YYMergeEncodingSettings");
struct State {
    done: bool,
    accepted: bool,
}
unsafe extern "system" fn procedure(h: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        let state = GetWindowLongPtrW(h, GWLP_USERDATA) as *mut State;
        if !state.is_null()
            && (message == WM_CLOSE || message == WM_COMMAND && matches!(w.0 & 0xffff, 1 | 2))
        {
            (*state).done = true;
            (*state).accepted = message == WM_COMMAND && w.0 & 0xffff == 1;
            return LRESULT(0);
        }
        DefWindowProcW(h, message, w, l)
    }
}
fn value(h: HWND) -> Option<String> {
    unsafe {
        let mut buffer = vec![0u16; GetWindowTextLengthW(h).max(0) as usize + 1];
        let count = GetWindowTextW(h, &mut buffer);
        let value = String::from_utf16_lossy(&buffer[..count.max(0) as usize]);
        let value = value.trim();
        (!value.is_empty()).then(|| value.to_owned())
    }
}
pub(super) fn show() -> Result<()> {
    let Some((owner, font, options)) = app(|a| -> Result<_> {
        rules_ready(a)?;
        if a.loaded
            .as_ref()
            .is_some_and(|d| d.inputs[0].office.is_some())
        {
            return Err("Office比較では文字コードを指定できません".into());
        }
        let mut options = a.options.inputs.clone();
        if !options.active() {
            options.left_encoding = a.options.encoding.clone();
            options.right_encoding = a.options.encoding.clone();
        }
        Ok((a.hwnd, a.font, options))
    })
    .transpose()?
    else {
        return Ok(());
    };
    let result = dialog(owner, font, options, false)?;
    if let Some(inputs) = result {
        app(|a| {
            a.options.inputs = inputs;
            a.loaded = None;
            a.options.encoding = None;
            recompare_rules(a);
        });
    }
    Ok(())
}
fn dialog(
    owner: HWND,
    font: HFONT,
    options: InputOptions,
    smoke: bool,
) -> Result<Option<InputOptions>> {
    unsafe {
        let instance = win(GetModuleHandleW(None))?;
        let class = WNDCLASSW {
            lpfnWndProc: Some(procedure),
            hInstance: instance.into(),
            lpszClassName: CLASS,
            hCursor: win(LoadCursorW(None, IDC_ARROW))?,
            hbrBackground: HBRUSH((COLOR_BTNFACE.0 + 1) as usize as *mut _),
            ..Default::default()
        };
        RegisterClassW(&class);
        let unit = |n| n * GetDpiForWindow(owner) as i32 / 96;
        let h = win(CreateWindowExW(
            WS_EX_DLGMODALFRAME,
            CLASS,
            w!("入力の文字コード"),
            WS_CAPTION | WS_SYSMENU,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            unit(640),
            unit(320),
            Some(owner),
            None,
            Some(instance.into()),
            None,
        ))?;
        // Destruction and owner restoration also run after control/message errors.
        struct Guard(HWND, HWND);
        impl Drop for Guard {
            fn drop(&mut self) {
                unsafe {
                    SetWindowLongPtrW(self.0, GWLP_USERDATA, 0);
                    let _ = EnableWindow(self.1, true);
                    let _ = DestroyWindow(self.0);
                    let _ = SetFocus(Some(self.1));
                }
            }
        }
        let _guard = Guard(h, owner);
        let mut state = Box::new(State {
            done: false,
            accepted: false,
        });
        SetWindowLongPtrW(h, GWLP_USERDATA, (&mut *state as *mut State) as isize);
        let controls = [
            child(
                h,
                w!("STATIC"),
                "左の文字コード（空欄はBOM検出／UTF-8）",
                10,
                WINDOW_STYLE(0),
            )?,
            child(
                h,
                w!("COMBOBOX"),
                "",
                11,
                WS_TABSTOP | WS_VSCROLL | WINDOW_STYLE(CBS_DROPDOWN as u32),
            )?,
            child(
                h,
                w!("STATIC"),
                "右の文字コード（固定長例: IBM-939/fixed:80）",
                12,
                WINDOW_STYLE(0),
            )?,
            child(
                h,
                w!("COMBOBOX"),
                "",
                13,
                WS_TABSTOP | WS_VSCROLL | WINDOW_STYLE(CBS_DROPDOWN as u32),
            )?,
            child(
                h,
                w!("BUTTON"),
                "文字コード・BOMの違いを無視する",
                14,
                WS_TABSTOP | WINDOW_STYLE(BS_AUTOCHECKBOX as u32),
            )?,
            child(
                h,
                w!("STATIC"),
                "読取専用。不正バイト・制御文字を可視化します。\nパック十進数の数値解釈は行いません。",
                15,
                WINDOW_STYLE(0),
            )?,
            child(
                h,
                w!("BUTTON"),
                "適用",
                1,
                WS_TABSTOP | WINDOW_STYLE(BS_DEFPUSHBUTTON as u32),
            )?,
            child(h, w!("BUTTON"), "キャンセル", 2, WS_TABSTOP)?,
        ];
        for (control, (x, y, w, h)) in controls.iter().zip([
            (12, 8, 600, 22),
            (12, 32, 600, 200),
            (12, 64, 600, 22),
            (12, 88, 600, 200),
            (12, 122, 600, 26),
            (12, 154, 600, 48),
            (388, 220, 100, 28),
            (500, 220, 112, 28),
        ]) {
            SendMessageW(
                *control,
                WM_SETFONT,
                Some(WPARAM(font.0 as usize)),
                Some(LPARAM(1)),
            );
            let _ = MoveWindow(*control, unit(x), unit(y), unit(w), unit(h), true);
        }
        for combo in [controls[1], controls[3]] {
            for name in [
                "UTF-8",
                "UTF-16LE",
                "UTF-16BE",
                "CP932",
                "EUC-JP",
                "ISO-2022-JP",
                "IBM-037",
                "IBM-500",
                "IBM-1047",
                "IBM-290",
                "IBM-1027",
                "IBM-930",
                "IBM-939",
                "IBM-1390",
                "IBM-1399",
                "IBM-939/fixed:80",
            ] {
                SendMessageW(
                    combo,
                    CB_ADDSTRING,
                    None,
                    Some(LPARAM(HSTRING::from(name).as_ptr() as isize)),
                );
            }
        }
        text(controls[1], options.left_encoding.as_deref().unwrap_or(""));
        text(controls[3], options.right_encoding.as_deref().unwrap_or(""));
        SendMessageW(
            controls[4],
            BM_SETCHECK,
            Some(WPARAM(usize::from(options.ignore_encoding))),
            None,
        );
        let _ = EnableWindow(owner, false);
        let _ = ShowWindow(h, SW_SHOW);
        let _ = SetFocus(Some(controls[1]));
        if smoke {
            SendMessageW(h, WM_COMMAND, Some(WPARAM(1)), None);
        }
        let mut message = MSG::default();
        while !state.done {
            let status = GetMessageW(&mut message, None, 0, 0).0;
            if status == -1 {
                return Err("文字コード設定のメッセージ処理に失敗しました".into());
            }
            if status == 0 {
                PostQuitMessage(message.wParam.0 as i32);
                return Ok(None);
            }
            if !IsDialogMessageW(h, &message).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            if state.accepted {
                for combo in [controls[1], controls[3]] {
                    if let Some(name) = value(combo)
                        && !yy_merge_core::review::valid_encoding(&name)
                    {
                        let _ = MessageBoxW(
                            Some(h),
                            &HSTRING::from(format!("未対応の文字コード: {name}")),
                            w!("文字コード"),
                            MB_OK | MB_ICONERROR,
                        );
                        state.accepted = false;
                        state.done = false;
                        break;
                    }
                }
            }
        }
        Ok(state.accepted.then(|| InputOptions {
            left_encoding: value(controls[1]),
            right_encoding: value(controls[3]),
            ignore_encoding: SendMessageW(controls[4], BM_GETCHECK, None, None).0 == 1,
        }))
    }
}

pub(super) fn smoke(a: &mut App, dir: &std::path::Path) -> Result<()> {
    let original = a.options.clone();
    let left = dir.join("encoding-left.dat");
    let right = dir.join("encoding-right.dat");
    std::fs::write(&left, "日本語\n").map_err(|e| e.to_string())?;
    let bytes = [0x93, 0xfa, 0x96, 0x7b, 0x8c, 0xea, 0x0a];
    std::fs::write(&right, bytes).map_err(|e| e.to_string())?;
    let inputs = dialog(
        a.hwnd,
        a.font,
        InputOptions {
            left_encoding: Some("UTF-8".into()),
            right_encoding: Some("CP932".into()),
            ignore_encoding: true,
        },
        true,
    )?
    .ok_or("encoding dialog cancelled")?;
    a.options = Gui {
        left: Some(left.clone()),
        right: Some(right.clone()),
        inputs,
        ..Default::default()
    };
    begin(a);
    await_rule_smoke(a)?;
    if a.exit != 0
        || a.loaded.as_ref().unwrap().merged.is_some()
        || replace_view::command(a, REPLACE_OPEN).is_ok()
    {
        return Err("mixed encoding comparison not equivalent/read-only".into());
    }
    set_rule(a, RULE_CASE)?;
    await_rule_smoke(a)?;
    if a.exit != 0 || a.loaded.as_ref().unwrap().inputs[1].encoding.spec() != "CP932" {
        return Err("encoding lost on rule reload".into());
    }
    let data = a.loaded.as_ref().unwrap();
    let c = Cancellation::new();
    let state = yy_merge_core::comparison_session::Session::capture(
        [&data.inputs[0], &data.inputs[1]],
        data.rules,
        Default::default(),
        &c,
    )?;
    std::fs::write(dir.join("encoding-session.json"), state.json(&c)?)
        .map_err(|e| e.to_string())?;
    begin_options(
        a,
        Gui {
            comparison_session: Some(dir.join("encoding-session.json")),
            ..Default::default()
        },
    );
    await_rule_smoke(a)?;
    if !a.options.inputs.ignore_encoding || a.exit != 0 {
        return Err("encoding session not restored".into());
    }
    set_rule(a, RULE_CASE)?;
    await_rule_smoke(a)?;
    if a.exit != 0 {
        return Err("encoding lost after session reload".into());
    }
    std::fs::write(&left, [0x12, 0x3c, 0, 0xff]).map_err(|e| e.to_string())?;
    std::fs::write(&right, [0x12, 0x3c, 0, 0xfe]).map_err(|e| e.to_string())?;
    a.options.inputs.right_encoding = Some("UTF-8".into());
    begin(a);
    await_rule_smoke(a)?;
    if a.exit != 1
        || !a.editors[0].text().contains("\\xFF")
        || !a.editors[1].text().contains("\\xFE")
    {
        return Err("binary bytes hidden or rejected".into());
    }
    for i in 0..2 {
        win(a.editors[i].offscreen(480, 220))?;
    }
    if std::fs::read(&left).map_err(|e| e.to_string())? != [0x12, 0x3c, 0, 0xff] {
        return Err("byte review changed original".into());
    }
    a.options = original;
    begin(a);
    await_rule_smoke(a)?;
    std::fs::write(dir.join("encoding-smoke.txt"), "PASS: native encoding dialog, UTF-8/CP932 equivalence, read-only panes, rule reload, comparison session restore/reload, packed/invalid bytes visible, originals preserved\n").map_err(|e| e.to_string())
}
