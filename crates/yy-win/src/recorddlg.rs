//! EBCDIC のレコードの区切り方を尋ねるダイアログ（03 章 3.3）。
//!
//! EBCDIC のファイルは改行を含まない固定長レコードであることが多いため、EBCDIC を選んで
//! 開く・保存するときに「改行 NL (0x15) / 改行 LF (0x25) / 固定長」を選んでもらう。

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Controls::{BST_CHECKED, CheckRadioButton, IsDlgButtonChecked};
use windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::HSTRING;
use yy_encoding::{Encoding, Records};

use crate::goto::{CLASS_BUTTON, CLASS_EDIT, CLASS_STATIC, Template};

const ID_LABEL: u16 = 100;
const ID_NL: u16 = 110;
const ID_LF: u16 = 111;
const ID_FIXED: u16 = 112;
const ID_LENGTH: u16 = 113;
const ID_UNIT: u16 = 114;
const IDOK_: u16 = 1;
const IDCANCEL_: u16 = 2;
/// 固定長レコードの長さの上限
const MAX_LENGTH: u32 = 1 << 20;

struct State {
    prompt: String,
    records: Records,
}

fn build_template() -> Template {
    let mut t = Template::dialog("レコードの区切り方", 220, 108);
    t.item(0, 7, 7, 206, 18, ID_LABEL, CLASS_STATIC, "");
    t.item(
        (WS_GROUP | WS_TABSTOP).0 | BS_AUTORADIOBUTTON as u32,
        14,
        28,
        190,
        11,
        ID_NL,
        CLASS_BUTTON,
        "改行 NL (0x15)  … z/OS のテキスト",
    );
    t.item(
        BS_AUTORADIOBUTTON as u32,
        14,
        41,
        190,
        11,
        ID_LF,
        CLASS_BUTTON,
        "改行 LF (0x25)",
    );
    t.item(
        BS_AUTORADIOBUTTON as u32,
        14,
        54,
        80,
        11,
        ID_FIXED,
        CLASS_BUTTON,
        "固定長レコード:",
    );
    t.item(
        (WS_GROUP | WS_TABSTOP | WS_BORDER).0 | (ES_NUMBER | ES_AUTOHSCROLL) as u32,
        96,
        53,
        50,
        13,
        ID_LENGTH,
        CLASS_EDIT,
        "",
    );
    t.item(0, 150, 55, 40, 10, ID_UNIT, CLASS_STATIC, "バイト");
    t.item(
        (WS_GROUP | WS_TABSTOP).0 | BS_DEFPUSHBUTTON as u32,
        109,
        86,
        50,
        14,
        IDOK_,
        CLASS_BUTTON,
        "OK",
    );
    t.item(
        WS_TABSTOP.0 | BS_PUSHBUTTON as u32,
        163,
        86,
        50,
        14,
        IDCANCEL_,
        CLASS_BUTTON,
        "キャンセル",
    );
    t
}

/// EBCDIC の `encoding` のレコードの区切り方を尋ねる（`encoding` の区切り方を初期値にする）。
/// EBCDIC でなければそのまま返す。キャンセルされたら `None`。
pub(crate) fn ask_records(owner: HWND, encoding: Encoding, purpose: &str) -> Option<Encoding> {
    let current = encoding.records()?;
    let aligned = build_template().aligned();
    let mut state = State {
        prompt: format!(
            "{} を{purpose}ときの、レコード（行）の区切り方:",
            encoding.name()
        ),
        records: current,
    };
    let r = unsafe {
        DialogBoxIndirectParamW(
            None,
            aligned.as_ptr() as *const DLGTEMPLATE,
            Some(owner),
            Some(dialog_proc),
            LPARAM(&mut state as *mut State as isize),
        )
    };
    (r == IDOK_ as isize).then(|| encoding.with_records(state.records))
}

/// EBCDIC なら区切り方を尋ねる。それ以外はそのまま。
pub(crate) fn confirm(owner: HWND, encoding: Encoding, purpose: &str) -> Option<Encoding> {
    if encoding.records().is_some() {
        ask_records(owner, encoding, purpose)
    } else {
        Some(encoding)
    }
}

fn update_enabled(hwnd: HWND) {
    unsafe {
        let fixed = IsDlgButtonChecked(hwnd, ID_FIXED as i32) == BST_CHECKED.0;
        for id in [ID_LENGTH, ID_UNIT] {
            if let Ok(w) = GetDlgItem(Some(hwnd), id as i32) {
                let _ = EnableWindow(w, fixed);
            }
        }
    }
}

extern "system" fn dialog_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> isize {
    unsafe {
        match msg {
            WM_INITDIALOG => {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, lparam.0);
                let state = &*(lparam.0 as *const State);
                let _ =
                    SetDlgItemTextW(hwnd, ID_LABEL as i32, &HSTRING::from(state.prompt.as_str()));
                let (radio, length) = match state.records {
                    Records::Nl => (ID_NL, 80),
                    Records::Lf => (ID_LF, 80),
                    Records::Fixed(n) => (ID_FIXED, n),
                };
                let _ = CheckRadioButton(hwnd, ID_NL as i32, ID_FIXED as i32, radio as i32);
                let _ = SetDlgItemTextW(hwnd, ID_LENGTH as i32, &HSTRING::from(length.to_string()));
                update_enabled(hwnd);
                1
            }
            WM_COMMAND => {
                let id = crate::loword(wparam.0) as u16;
                match id {
                    ID_NL | ID_LF | ID_FIXED => {
                        update_enabled(hwnd);
                        0
                    }
                    IDOK_ => {
                        let state = &mut *(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State);
                        let checked =
                            |id: u16| IsDlgButtonChecked(hwnd, id as i32) == BST_CHECKED.0;
                        state.records = if checked(ID_FIXED) {
                            let mut buf = [0u16; 16];
                            let n = GetDlgItemTextW(hwnd, ID_LENGTH as i32, &mut buf) as usize;
                            match String::from_utf16_lossy(&buf[..n]).trim().parse::<u32>() {
                                Ok(len) if (1..=MAX_LENGTH).contains(&len) => Records::Fixed(len),
                                _ => {
                                    let _ = MessageBoxW(
                                        Some(hwnd),
                                        &HSTRING::from(format!(
                                            "レコード長は 1 〜 {MAX_LENGTH} で指定してください。"
                                        )),
                                        &HSTRING::from("yyeditor"),
                                        MB_OK | MB_ICONWARNING,
                                    );
                                    return 1;
                                }
                            }
                        } else if checked(ID_LF) {
                            Records::Lf
                        } else {
                            Records::Nl
                        };
                        let _ = EndDialog(hwnd, IDOK_ as isize);
                        1
                    }
                    IDCANCEL_ => {
                        let _ = EndDialog(hwnd, IDCANCEL_ as isize);
                        1
                    }
                    _ => 0,
                }
            }
            _ => 0,
        }
    }
}
