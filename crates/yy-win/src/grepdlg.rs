//! 「ファイルから検索（Grep）」ダイアログ。

use std::path::PathBuf;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree};
use windows::Win32::UI::Shell::{
    FOS_PICKFOLDERS, FileOpenDialog, IFileOpenDialog, SIGDN_FILESYSPATH,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::HSTRING;

use crate::goto::{CLASS_BUTTON, CLASS_EDIT, CLASS_STATIC, Template};

const ID_PATTERN: u16 = 100;
const ID_FILES: u16 = 101;
const ID_DIR: u16 = 102;
const ID_BROWSE: u16 = 103;
const ID_CASE: u16 = 104;
const ID_WORD: u16 = 105;
const ID_REGEX: u16 = 106;
const ID_RECURSIVE: u16 = 107;
const IDOK_: u16 = 1;
const IDCANCEL_: u16 = 2;

/// ダイアログで指定する内容。
#[derive(Clone, Debug, Default)]
pub(crate) struct GrepRequest {
    pub pattern: String,
    pub files: String,
    pub dir: String,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
    pub recursive: bool,
}

fn build_template() -> Template {
    let mut t = Template::dialog("ファイルから検索 (Grep)", 13, 260, 118);
    let edit = (WS_BORDER | WS_TABSTOP).0 | ES_AUTOHSCROLL as u32;
    let check = WS_TABSTOP.0 | BS_AUTOCHECKBOX as u32;
    t.item(0, 7, 8, 60, 10, 0, CLASS_STATIC, "検索する文字列:");
    t.item(edit, 70, 6, 183, 13, ID_PATTERN, CLASS_EDIT, "");
    t.item(0, 7, 26, 60, 10, 0, CLASS_STATIC, "ファイル名:");
    t.item(edit, 70, 24, 183, 13, ID_FILES, CLASS_EDIT, "");
    t.item(0, 7, 44, 60, 10, 0, CLASS_STATIC, "フォルダ:");
    t.item(edit, 70, 42, 140, 13, ID_DIR, CLASS_EDIT, "");
    t.item(
        WS_TABSTOP.0 | BS_PUSHBUTTON as u32,
        214,
        41,
        39,
        14,
        ID_BROWSE,
        CLASS_BUTTON,
        "参照...",
    );
    t.item(
        check,
        7,
        62,
        110,
        10,
        ID_CASE,
        CLASS_BUTTON,
        "大文字と小文字を区別",
    );
    t.item(check, 120, 62, 60, 10, ID_WORD, CLASS_BUTTON, "単語単位");
    t.item(check, 185, 62, 68, 10, ID_REGEX, CLASS_BUTTON, "正規表現");
    t.item(
        check,
        7,
        76,
        110,
        10,
        ID_RECURSIVE,
        CLASS_BUTTON,
        "サブフォルダも検索",
    );
    t.item(
        WS_TABSTOP.0 | BS_DEFPUSHBUTTON as u32,
        147,
        98,
        50,
        14,
        IDOK_,
        CLASS_BUTTON,
        "検索",
    );
    t.item(
        WS_TABSTOP.0 | BS_PUSHBUTTON as u32,
        203,
        98,
        50,
        14,
        IDCANCEL_,
        CLASS_BUTTON,
        "キャンセル",
    );
    t
}

/// Grep の条件を尋ねる。キャンセルされたら `None`。
pub(crate) fn prompt(owner: HWND, initial: &GrepRequest) -> Option<GrepRequest> {
    let aligned = build_template().aligned();
    let mut state = initial.clone();
    let r = unsafe {
        DialogBoxIndirectParamW(
            None,
            aligned.as_ptr() as *const DLGTEMPLATE,
            Some(owner),
            Some(dialog_proc),
            LPARAM(&mut state as *mut GrepRequest as isize),
        )
    };
    (r == IDOK_ as isize && !state.pattern.is_empty()).then_some(state)
}

fn get_text(hwnd: HWND, id: u16) -> String {
    unsafe {
        let Ok(item) = GetDlgItem(Some(hwnd), id as i32) else {
            return String::new();
        };
        let len = GetWindowTextLengthW(item).max(0) as usize;
        let mut buf = vec![0u16; len + 1];
        let n = GetWindowTextW(item, &mut buf).max(0) as usize;
        String::from_utf16_lossy(&buf[..n])
    }
}

fn set_check(hwnd: HWND, id: u16, on: bool) {
    unsafe {
        SendDlgItemMessageW(hwnd, id as i32, BM_SETCHECK, WPARAM(on as usize), LPARAM(0));
    }
}

fn checked(hwnd: HWND, id: u16) -> bool {
    unsafe { SendDlgItemMessageW(hwnd, id as i32, BM_GETCHECK, WPARAM(0), LPARAM(0)).0 == 1 }
}

/// フォルダを選ぶ。
pub(crate) fn browse_folder(owner: HWND) -> Option<PathBuf> {
    unsafe {
        let dialog: IFileOpenDialog =
            CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let opts = dialog.GetOptions().ok()?;
        dialog.SetOptions(opts | FOS_PICKFOLDERS).ok()?;
        dialog.Show(Some(owner)).ok()?;
        let item = dialog.GetResult().ok()?;
        let name = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let path = name.to_string().ok();
        CoTaskMemFree(Some(name.0 as *const _));
        path.map(PathBuf::from)
    }
}

extern "system" fn dialog_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> isize {
    unsafe {
        match msg {
            WM_INITDIALOG => {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, lparam.0);
                let s = &*(lparam.0 as *const GrepRequest);
                let set = |id: u16, t: &str| {
                    let _ = SetDlgItemTextW(hwnd, id as i32, &HSTRING::from(t));
                };
                set(ID_PATTERN, &s.pattern);
                set(ID_FILES, &s.files);
                set(ID_DIR, &s.dir);
                set_check(hwnd, ID_CASE, s.case_sensitive);
                set_check(hwnd, ID_WORD, s.whole_word);
                set_check(hwnd, ID_REGEX, s.regex);
                set_check(hwnd, ID_RECURSIVE, s.recursive);
                1
            }
            WM_COMMAND => {
                let id = crate::loword(wparam.0) as u16;
                match id {
                    IDOK_ => {
                        let s = &mut *(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut GrepRequest);
                        s.pattern = get_text(hwnd, ID_PATTERN);
                        s.files = get_text(hwnd, ID_FILES);
                        s.dir = get_text(hwnd, ID_DIR);
                        s.case_sensitive = checked(hwnd, ID_CASE);
                        s.whole_word = checked(hwnd, ID_WORD);
                        s.regex = checked(hwnd, ID_REGEX);
                        s.recursive = checked(hwnd, ID_RECURSIVE);
                        let _ = EndDialog(hwnd, IDOK_ as isize);
                        1
                    }
                    IDCANCEL_ => {
                        let _ = EndDialog(hwnd, IDCANCEL_ as isize);
                        1
                    }
                    ID_BROWSE => {
                        if let Some(p) = browse_folder(hwnd) {
                            let _ = SetDlgItemTextW(
                                hwnd,
                                ID_DIR as i32,
                                &HSTRING::from(p.display().to_string()),
                            );
                        }
                        1
                    }
                    _ => 0,
                }
            }
            _ => 0,
        }
    }
}
