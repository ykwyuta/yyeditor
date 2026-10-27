use super::*;
use std::cell::Cell;

#[derive(Clone)]
pub(super) struct Settings {
    pub format: usize,
    pub delimiter: String,
    pub charset: String,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            format: 0,
            delimiter: "|".into(),
            charset: "MS932".into(),
        }
    }
}
impl Settings {
    fn validate(&self) -> yy_db_core::Result<()> {
        if self.format == 2 {
            let mut chars = self.delimiter.chars();
            let first = chars.next().ok_or("区切り文字を1文字入力してください")?;
            if chars.next().is_some() || matches!(first, '\r' | '\n' | '"') {
                return Err("区切り文字は引用符・改行以外の1文字です".into());
            }
        }
        if self.format == 4 && yy_cobol::Charset::from_name(&self.charset).is_none() {
            return Err(
                "固定長ファイルの文字コードはMS932またはCCSID930等を指定してください".into(),
            );
        }
        Ok(())
    }
}
struct Dialog {
    format: HWND,
    delimiter: HWND,
    charset: HWND,
    hint: HWND,
    settings: Settings,
    active_format: usize,
    result: Option<Settings>,
    done: Cell<bool>,
}
fn refresh(s: &mut Dialog) {
    let index = unsafe { send_message(s.format, CB_GETCURSEL, WPARAM(0), LPARAM(0)).0 };
    s.active_format = index.max(0) as usize;
    unsafe {
        let _ = EnableWindow(s.delimiter, index == 2);
        let _ = EnableWindow(s.charset, index == 4);
    }
    set_text(
        s.charset,
        if index == 4 {
            &s.settings.charset
        } else if index == 3 {
            "Parquet形式に準拠"
        } else {
            "UTF-8"
        },
    );
    set_text(
        s.hint,
        match index {
            4 => {
                "次にデータファイル、続いてCOBOLコピー句（.cbl）を選択します。文字コードは固定長ファイルに適用されます。"
            }
            3 => "Parquetは列の型を保持します。区切り文字とテキストの文字コード指定は使いません。",
            _ => {
                "見出し行あり・UTF-8です。引用符と項目内改行はCSVの規則で扱います。任意区切りの場合だけ区切り文字を1文字指定します。"
            }
        },
    );
}
unsafe extern "system" fn proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    // Updating child controls sends synchronous notifications back to this window.
    // Only borrow dialog state for commands that actually operate on it.
    let id = w.0 & 0xffff;
    if m != WM_CLOSE
        && !(m == WM_COMMAND
            && (matches!(id, 1 | 2) || (id == 101 && w.0 >> 16 == CBN_SELCHANGE as usize)))
    {
        return unsafe { DefWindowProcW(h, m, w, l) };
    }
    let p = unsafe { GetWindowLongPtrW(h, GWLP_USERDATA) } as *mut Dialog;
    if !p.is_null() {
        let s = unsafe { &mut *p };
        if m == WM_CLOSE || (m == WM_COMMAND && w.0 & 0xffff == 2) {
            s.done.set(true);
            unsafe {
                PostMessageW(Some(h), WM_NULL, WPARAM(0), LPARAM(0)).ok();
            }
            return LRESULT(0);
        }
        if m == WM_COMMAND {
            if w.0 & 0xffff == 101 && w.0 >> 16 == CBN_SELCHANGE as usize {
                if s.active_format == 4 {
                    s.settings.charset = text(s.charset);
                }
                refresh(s);
                return LRESULT(0);
            }
            if w.0 & 0xffff == 1 {
                let settings = Settings {
                    format: unsafe {
                        send_message(s.format, CB_GETCURSEL, WPARAM(0), LPARAM(0))
                            .0
                            .max(0) as usize
                    },
                    delimiter: text(s.delimiter),
                    charset: if s.active_format == 4 {
                        text(s.charset)
                    } else {
                        s.settings.charset.clone()
                    },
                };
                if let Err(error) = settings.validate() {
                    unsafe {
                        MessageBoxW(
                            Some(h),
                            &HSTRING::from(error),
                            w!("ファイル形式の設定"),
                            MB_OK | MB_ICONWARNING,
                        );
                    }
                } else {
                    s.result = Some(settings);
                    s.done.set(true);
                }
                unsafe {
                    PostMessageW(Some(h), WM_NULL, WPARAM(0), LPARAM(0)).ok();
                }
                return LRESULT(0);
            }
        }
    }
    unsafe { DefWindowProcW(h, m, w, l) }
}
pub(super) fn edit(
    parent: HWND,
    export: bool,
    settings: Settings,
    font: HFONT,
    dpi: u32,
) -> Option<Settings> {
    unsafe {
        let instance = GetModuleHandleW(None).ok()?;
        RegisterClassW(&WNDCLASSW {
            hInstance: instance.into(),
            lpszClassName: w!("YYDbTransfer"),
            lpfnWndProc: Some(proc),
            hbrBackground: GetSysColorBrush(COLOR_BTNFACE),
            ..Default::default()
        });
        let px = |v: i32| v * dpi as i32 / 96;
        let mut r = RECT::default();
        GetWindowRect(parent, &mut r).ok()?;
        let h = CreateWindowExW(
            WS_EX_DLGMODALFRAME,
            w!("YYDbTransfer"),
            if export {
                w!("結果をエクスポート")
            } else {
                w!("データをインポート")
            },
            WS_CAPTION | WS_SYSMENU | WS_POPUP,
            r.left + px(30),
            r.top + px(20),
            px(640),
            px(360),
            Some(parent),
            None,
            Some(instance.into()),
            None,
        )
        .ok()?;
        let control = |class: PCWSTR,
                       value: &str,
                       id: usize,
                       style: u32,
                       x,
                       y,
                       width,
                       height|
         -> Option<HWND> {
            let child = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                class,
                &HSTRING::from(value),
                WS_CHILD | WS_VISIBLE | WINDOW_STYLE(style),
                px(x),
                px(y),
                px(width),
                px(height),
                Some(h),
                Some(HMENU(id as *mut _)),
                Some(instance.into()),
                None,
            )
            .ok()?;
            send_message(child, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(0));
            Some(child)
        };
        control(
            w!("STATIC"),
            if export {
                "選択中の結果セットをファイルへ出力します。"
            } else {
                "ファイル名を表名として、接続中のDBへ取り込みます。"
            },
            10,
            0,
            16,
            16,
            600,
            26,
        )?;
        control(w!("STATIC"), "ファイル形式", 11, 0, 16, 58, 150, 24)?;
        let format = control(
            w!("COMBOBOX"),
            "",
            101,
            CBS_DROPDOWNLIST as u32 | WS_TABSTOP.0,
            176,
            54,
            436,
            220,
        )?;
        for label in [
            "CSV (RFC 4180)",
            "TSV",
            "任意区切り",
            "Parquet",
            "COBOL固定長",
        ] {
            send_message(
                format,
                CB_ADDSTRING,
                WPARAM(0),
                LPARAM(wide(label).as_ptr() as isize),
            );
        }
        send_message(format, CB_SETCURSEL, WPARAM(settings.format), LPARAM(0));
        control(w!("STATIC"), "区切り文字", 12, 0, 16, 96, 150, 24)?;
        let delimiter = control(
            w!("EDIT"),
            &settings.delimiter,
            102,
            WS_BORDER.0 | WS_TABSTOP.0 | ES_AUTOHSCROLL as u32,
            176,
            92,
            120,
            28,
        )?;
        control(w!("STATIC"), "文字コード", 13, 0, 16, 134, 150, 24)?;
        let charset = control(
            w!("EDIT"),
            "",
            103,
            WS_BORDER.0 | WS_TABSTOP.0 | ES_AUTOHSCROLL as u32,
            176,
            130,
            436,
            28,
        )?;
        let hint = control(w!("STATIC"), "", 14, 0, 16, 176, 596, 64)?;
        control(
            w!("BUTTON"),
            "ファイルを選択…",
            1,
            BS_DEFPUSHBUTTON as u32 | WS_TABSTOP.0,
            304,
            266,
            168,
            28,
        )?;
        control(
            w!("BUTTON"),
            "キャンセル",
            2,
            WS_TABSTOP.0,
            484,
            266,
            128,
            28,
        )?;
        let mut s = Dialog {
            format,
            delimiter,
            charset,
            hint,
            settings,
            active_format: 0,
            result: None,
            done: Cell::new(false),
        };
        refresh(&mut s);
        SetWindowLongPtrW(h, GWLP_USERDATA, &mut s as *mut _ as isize);
        let _ = EnableWindow(parent, false);
        let _ = ShowWindow(h, SW_SHOW);
        SetFocus(Some(format)).ok();
        let mut msg = MSG::default();
        while !s.done.get() {
            let code = GetMessageW(&mut msg, None, 0, 0).0;
            if code <= 0 {
                if code == 0 {
                    PostQuitMessage(msg.wParam.0 as i32);
                }
                break;
            }
            if !IsDialogMessageW(h, &msg).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        let _ = EnableWindow(parent, true);
        DestroyWindow(h).ok();
        let _ = SetForegroundWindow(parent);
        s.result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_only_options_used_by_the_selected_format() {
        let mut s = Settings {
            format: 2,
            delimiter: "｜".into(),
            charset: "unused".into(),
        };
        assert!(s.validate().is_ok());
        for invalid in ["", "ab", "\n", "\r", "\""] {
            s.delimiter = invalid.into();
            assert!(s.validate().is_err());
        }
        s.format = 0;
        assert!(s.validate().is_ok());
        s.format = 4;
        assert!(s.validate().is_err());
        s.charset = "MS932".into();
        assert!(s.validate().is_ok());
    }
}
