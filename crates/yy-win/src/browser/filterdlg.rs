//! 広告ブロックのフィルタリストの一覧の編集（20 章 6）。入・切（ダブルクリックか「入・切」）、追加、
//! 削除、既定に戻す。状態（規則の数・更新した時刻・失敗）も出す。

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::HSTRING;
use yy_browser::FilterList;
use yy_browser::profiles::default_filter_lists;

use crate::goto::{CLASS_BUTTON, CLASS_STATIC, Template};

const CLASS_LISTBOX: u16 = 0x0083;

const ID_LIST: u16 = 100;
const ID_TOGGLE: u16 = 101;
const ID_ADD: u16 = 102;
const ID_REMOVE: u16 = 103;
const ID_DEFAULTS: u16 = 104;
const ID_DETAIL: u16 = 105;
const IDOK_: u16 = 1;
const IDCANCEL_: u16 = 2;

struct State {
    lists: Vec<FilterList>,
    done: bool,
}

/// 一覧を編集する。OK なら編集後の一覧。
pub(super) fn edit(owner: HWND, lists: Vec<FilterList>) -> Option<Vec<FilterList>> {
    let mut t = Template::dialog("フィルタリスト", 340, 220);
    t.item(
        0,
        7,
        7,
        326,
        10,
        0xFFFF,
        CLASS_STATIC,
        "チェック（☑）したリストで広告・追跡を止めます。ダブルクリックで入・切。",
    );
    t.item(
        (WS_BORDER | WS_TABSTOP | WS_VSCROLL).0 | (LBS_NOTIFY | LBS_NOINTEGRALHEIGHT) as u32,
        7,
        20,
        270,
        150,
        ID_LIST,
        CLASS_LISTBOX,
        "",
    );
    let button = |t: &mut Template, y: i16, id: u16, text: &str| {
        t.item(
            WS_TABSTOP.0 | BS_PUSHBUTTON as u32,
            283,
            y,
            50,
            14,
            id,
            CLASS_BUTTON,
            text,
        );
    };
    button(&mut t, 20, ID_TOGGLE, "入・切");
    button(&mut t, 38, ID_ADD, "追加...");
    button(&mut t, 56, ID_REMOVE, "削除");
    button(&mut t, 80, ID_DEFAULTS, "既定に戻す");
    t.item(0, 7, 174, 326, 20, ID_DETAIL, CLASS_STATIC, "");
    t.item(
        WS_TABSTOP.0 | BS_DEFPUSHBUTTON as u32,
        229,
        199,
        50,
        14,
        IDOK_,
        CLASS_BUTTON,
        "保存",
    );
    t.item(
        WS_TABSTOP.0 | BS_PUSHBUTTON as u32,
        283,
        199,
        50,
        14,
        IDCANCEL_,
        CLASS_BUTTON,
        "キャンセル",
    );
    let aligned = t.aligned();
    let mut state = State { lists, done: false };
    unsafe {
        DialogBoxIndirectParamW(
            None,
            aligned.as_ptr() as *const DLGTEMPLATE,
            Some(owner),
            Some(dialog_proc),
            LPARAM(&mut state as *mut State as isize),
        );
    }
    state.done.then_some(state.lists)
}

fn selected(dlg: HWND) -> Option<usize> {
    let i =
        unsafe { SendDlgItemMessageW(dlg, ID_LIST as i32, LB_GETCURSEL, WPARAM(0), LPARAM(0)).0 };
    (i >= 0).then_some(i as usize)
}

/// 一覧を作り直す（`sel` を選ぶ）。
fn fill(dlg: HWND, st: &State, sel: Option<usize>) {
    unsafe {
        SendDlgItemMessageW(dlg, ID_LIST as i32, LB_RESETCONTENT, WPARAM(0), LPARAM(0));
        for l in &st.lists {
            let s = HSTRING::from(format!(
                "{} {}　—　{}",
                if l.enabled { "☑" } else { "☐" },
                l.name,
                super::adblock::list_state(l)
            ));
            SendDlgItemMessageW(
                dlg,
                ID_LIST as i32,
                LB_ADDSTRING,
                WPARAM(0),
                LPARAM(s.as_ptr() as isize),
            );
        }
        if let Some(i) = sel.filter(|i| *i < st.lists.len()) {
            SendDlgItemMessageW(dlg, ID_LIST as i32, LB_SETCURSEL, WPARAM(i), LPARAM(0));
        }
    }
    show_detail(dlg, st);
}

fn show_detail(dlg: HWND, st: &State) {
    let text = selected(dlg)
        .and_then(|i| st.lists.get(i))
        .map(|l| l.url.clone())
        .unwrap_or_default();
    unsafe {
        let _ = SetDlgItemTextW(dlg, ID_DETAIL as i32, &HSTRING::from(text));
    }
}

extern "system" fn dialog_proc(dlg: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> isize {
    unsafe {
        match msg {
            WM_INITDIALOG => {
                SetWindowLongPtrW(dlg, GWLP_USERDATA, lparam.0);
                let st = &*(lparam.0 as *const State);
                fill(dlg, st, Some(0));
                1
            }
            WM_COMMAND => {
                let st = &mut *(GetWindowLongPtrW(dlg, GWLP_USERDATA) as *mut State);
                let id = (wparam.0 & 0xffff) as u16;
                let code = ((wparam.0 >> 16) & 0xffff) as u32;
                match id {
                    ID_LIST if code == LBN_SELCHANGE => show_detail(dlg, st),
                    ID_LIST if code == LBN_DBLCLK => toggle(dlg, st),
                    ID_TOGGLE => toggle(dlg, st),
                    ID_ADD => {
                        let Some(name) =
                            crate::goto::prompt_text(dlg, "フィルタリストを追加", "名前:", "")
                        else {
                            return 1;
                        };
                        let Some(url) = crate::goto::prompt_text(
                            dlg,
                            "フィルタリストを追加",
                            "URL（https://…）か、ローカルのファイルのパス（C:\\…\\list.txt）:",
                            "https://",
                        ) else {
                            return 1;
                        };
                        let l = FilterList::new(name.trim(), url.trim(), true);
                        match l.validate() {
                            Ok(()) => {
                                st.lists.push(l);
                                fill(dlg, st, Some(st.lists.len() - 1));
                            }
                            Err(e) => crate::util::error_box(dlg, &e),
                        }
                    }
                    ID_REMOVE => {
                        if let Some(i) = selected(dlg) {
                            st.lists.remove(i);
                            fill(dlg, st, Some(i.min(st.lists.len().saturating_sub(1))));
                        }
                    }
                    ID_DEFAULTS => {
                        st.lists = default_filter_lists();
                        fill(dlg, st, Some(0));
                    }
                    IDOK_ => {
                        st.done = true;
                        let _ = EndDialog(dlg, IDOK_ as isize);
                    }
                    IDCANCEL_ => {
                        let _ = EndDialog(dlg, IDCANCEL_ as isize);
                    }
                    _ => {}
                }
                1
            }
            _ => 0,
        }
    }
}

fn toggle(dlg: HWND, st: &mut State) {
    if let Some(i) = selected(dlg) {
        if let Some(l) = st.lists.get_mut(i) {
            l.enabled = !l.enabled;
        }
        fill(dlg, st, Some(i));
    }
}
