//! ブックマークの画面（19 章 4.1）: 追加・編集の画面、管理の画面、メニューの中身。
//!
//! 一覧は設定のフォルダの `bookmarks.toml`。ほかのウィンドウ（別のプロセス）も書き換えるので、変えるときは
//! そのつど読み直してから変えて保存する。

use std::path::PathBuf;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Controls::EM_SETSEL;
use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, SetFocus};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::HSTRING;
use yy_browser::bookmarks::{Bookmark, Bookmarks, Node};

use crate::goto::{CLASS_BUTTON, CLASS_EDIT, CLASS_STATIC, Template};

const CLASS_LISTBOX: u16 = 0x0083;
const CLASS_COMBO: u16 = 0x0085;
const IDOK_: u16 = 1;
const IDCANCEL_: u16 = 2;
const NO_ID: u16 = 0xFFFF;
/// メニューに出すブックマークの上限。
pub(super) const MENU_MAX: usize = 2000;

/// ブックマークのファイル（設定のフォルダの `bookmarks.toml`）。
pub(super) fn path() -> PathBuf {
    yy_config::config_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("bookmarks.toml")
}

/// 読む（読めなければ空）。
pub(super) fn load() -> Bookmarks {
    Bookmarks::load(&path()).unwrap_or_default()
}

/// 保存する（だめなら理由を出して `false`）。
pub(super) fn save(owner: HWND, list: &Bookmarks) -> bool {
    match list.save(&path()) {
        Ok(()) => true,
        Err(e) => {
            crate::util::error_box(owner, &format!("ブックマークを保存できません: {e}"));
            false
        }
    }
}

/// メニューの項目の文字列（`&` を重ね、長すぎれば切る）。
fn menu_label(s: &str) -> String {
    let mut t: String = s.chars().take(60).collect();
    if s.chars().count() > 60 {
        t.push('…');
    }
    t.replace('&', "&&")
}

/// メニューにブックマークを足す（フォルダはサブメニュー）。項目の ID は `base + 番号`。
pub(super) fn fill_menu(m: HMENU, list: &Bookmarks, base: u16) {
    fn add(m: HMENU, list: &Bookmarks, n: &Node, base: u16) {
        unsafe {
            for (name, child) in &n.folders {
                if let Ok(sub) = CreatePopupMenu() {
                    add(sub, list, child, base);
                    let _ = AppendMenuW(
                        m,
                        MF_POPUP,
                        sub.0 as usize,
                        &HSTRING::from(format!("📁 {}", menu_label(name))),
                    );
                }
            }
            for &i in n.items.iter().filter(|i| **i < MENU_MAX) {
                let _ = AppendMenuW(
                    m,
                    MF_STRING,
                    base as usize + i,
                    &HSTRING::from(menu_label(list.items[i].label())),
                );
            }
        }
    }
    let tree = list.tree();
    if tree.folders.is_empty() && tree.items.is_empty() {
        unsafe {
            let _ = AppendMenuW(
                m,
                MF_STRING | MF_GRAYED,
                0,
                &HSTRING::from("（ブックマークはまだありません）"),
            );
        }
        return;
    }
    add(m, list, &tree, base);
}

// ---- 共通 ---------------------------------------------------------------------------------

fn item(dlg: HWND, id: u16) -> HWND {
    unsafe { GetDlgItem(Some(dlg), id as i32).unwrap_or_default() }
}

fn get_text(dlg: HWND, id: u16) -> String {
    unsafe {
        let h = item(dlg, id);
        let n = GetWindowTextLengthW(h);
        let mut buf = vec![0u16; n as usize + 1];
        let got = GetWindowTextW(h, &mut buf) as usize;
        String::from_utf16_lossy(&buf[..got]).trim().to_owned()
    }
}

fn set_text(dlg: HWND, id: u16, s: &str) {
    unsafe {
        let _ = SetDlgItemTextW(dlg, id as i32, &HSTRING::from(s));
    }
}

fn state_of<'a, S>(dlg: HWND) -> &'a mut S {
    unsafe { &mut *(GetWindowLongPtrW(dlg, GWLP_USERDATA) as *mut S) }
}

fn run<S>(owner: HWND, t: Template, proc_: DLGPROC, state: &mut S) {
    let aligned = t.aligned();
    unsafe {
        DialogBoxIndirectParamW(
            None,
            aligned.as_ptr() as *const DLGTEMPLATE,
            Some(owner),
            proc_,
            LPARAM(state as *mut S as isize),
        );
    }
}

fn end(dlg: HWND, code: u16) {
    unsafe {
        let _ = EndDialog(dlg, code as isize);
    }
}

// ---- 追加・編集 ----------------------------------------------------------------------------

const E_TITLE: u16 = 100;
const E_URL: u16 = 101;
const E_FOLDER: u16 = 102;
const E_DELETE: u16 = 103;

/// 編集の画面の結果。
pub(super) enum EditResult {
    Save(Bookmark),
    Delete,
    Cancel,
}

struct EditState {
    bookmark: Bookmark,
    existing: bool,
    folders: Vec<String>,
    result: EditResult,
}

/// ブックマークを足す・直す画面。`existing` なら「削除」を使える。
pub(super) fn edit(
    owner: HWND,
    bookmark: Bookmark,
    existing: bool,
    folders: Vec<String>,
) -> EditResult {
    let title = if existing {
        "ブックマークの編集"
    } else {
        "ブックマークに追加"
    };
    let mut t = Template::dialog(title, 300, 92);
    let label =
        |t: &mut Template, y: i16, s: &str| t.item(0, 7, y + 2, 44, 10, NO_ID, CLASS_STATIC, s);
    label(&mut t, 7, "名前");
    t.item(
        (WS_BORDER | WS_TABSTOP).0 | ES_AUTOHSCROLL as u32,
        54,
        7,
        239,
        13,
        E_TITLE,
        CLASS_EDIT,
        "",
    );
    label(&mut t, 25, "URL");
    t.item(
        (WS_BORDER | WS_TABSTOP).0 | ES_AUTOHSCROLL as u32,
        54,
        25,
        239,
        13,
        E_URL,
        CLASS_EDIT,
        "",
    );
    label(&mut t, 43, "フォルダ");
    // 選ぶか、新しい名前を入れる
    t.item(
        (WS_TABSTOP | WS_VSCROLL).0 | (CBS_DROPDOWN | CBS_AUTOHSCROLL) as u32,
        54,
        43,
        239,
        120,
        E_FOLDER,
        CLASS_COMBO,
        "",
    );
    t.item(
        WS_TABSTOP.0 | BS_PUSHBUTTON as u32,
        7,
        71,
        50,
        14,
        E_DELETE,
        CLASS_BUTTON,
        "削除",
    );
    t.item(
        WS_TABSTOP.0 | BS_DEFPUSHBUTTON as u32,
        189,
        71,
        50,
        14,
        IDOK_,
        CLASS_BUTTON,
        "保存",
    );
    t.item(
        WS_TABSTOP.0 | BS_PUSHBUTTON as u32,
        243,
        71,
        50,
        14,
        IDCANCEL_,
        CLASS_BUTTON,
        "キャンセル",
    );
    let mut st = EditState {
        bookmark,
        existing,
        folders,
        result: EditResult::Cancel,
    };
    run(owner, t, Some(edit_proc), &mut st);
    st.result
}

extern "system" fn edit_proc(dlg: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> isize {
    match msg {
        WM_INITDIALOG => {
            unsafe { SetWindowLongPtrW(dlg, GWLP_USERDATA, lparam.0) };
            let st = state_of::<EditState>(dlg);
            set_text(dlg, E_TITLE, &st.bookmark.title);
            set_text(dlg, E_URL, &st.bookmark.url);
            unsafe {
                let none = HSTRING::from("（一番上）");
                SendDlgItemMessageW(
                    dlg,
                    E_FOLDER as i32,
                    CB_ADDSTRING,
                    WPARAM(0),
                    LPARAM(none.as_ptr() as isize),
                );
                for f in &st.folders {
                    let s = HSTRING::from(f.as_str());
                    SendDlgItemMessageW(
                        dlg,
                        E_FOLDER as i32,
                        CB_ADDSTRING,
                        WPARAM(0),
                        LPARAM(s.as_ptr() as isize),
                    );
                }
                let _ = EnableWindow(item(dlg, E_DELETE), st.existing);
                let _ = SetFocus(Some(item(dlg, E_TITLE)));
                SendDlgItemMessageW(dlg, E_TITLE as i32, EM_SETSEL, WPARAM(0), LPARAM(-1));
            }
            set_text(
                dlg,
                E_FOLDER,
                if st.bookmark.folder.is_empty() {
                    "（一番上）"
                } else {
                    &st.bookmark.folder
                },
            );
            0
        }
        WM_COMMAND => {
            let st = state_of::<EditState>(dlg);
            match (wparam.0 & 0xffff) as u16 {
                IDOK_ => {
                    // 組み合わせボックスで選んだ直後は、まだ欄の文字が変わっていないことがある
                    let sel = unsafe {
                        SendDlgItemMessageW(
                            dlg,
                            E_FOLDER as i32,
                            CB_GETCURSEL,
                            WPARAM(0),
                            LPARAM(0),
                        )
                        .0
                    };
                    let typed = get_text(dlg, E_FOLDER);
                    let folder = if sel > 0 && typed.is_empty() {
                        st.folders[sel as usize - 1].clone()
                    } else if typed == "（一番上）" {
                        String::new()
                    } else {
                        typed
                    };
                    let b = Bookmark::new(&get_text(dlg, E_TITLE), &get_text(dlg, E_URL), &folder);
                    match b.validate() {
                        Ok(()) => {
                            st.result = EditResult::Save(b);
                            end(dlg, IDOK_);
                        }
                        Err(e) => crate::util::error_box(dlg, &e),
                    }
                }
                E_DELETE => {
                    st.result = EditResult::Delete;
                    end(dlg, IDOK_);
                }
                IDCANCEL_ => end(dlg, IDCANCEL_),
                _ => {}
            }
            1
        }
        _ => 0,
    }
}

// ---- 管理 ----------------------------------------------------------------------------------

const M_LIST: u16 = 200;
const M_OPEN: u16 = 201;
const M_OPEN_TAB: u16 = 202;
const M_EDIT: u16 = 203;
const M_DELETE: u16 = 204;
const M_UP: u16 = 205;
const M_DOWN: u16 = 206;
const M_IMPORT: u16 = 207;
const M_EXPORT: u16 = 208;
const M_COUNT: u16 = 209;

struct ManageState {
    list: Bookmarks,
    /// 開く URL と、新しいタブで開くか
    open: Option<(String, bool)>,
}

/// ブックマークの管理の画面。開くものを選んだら（URL, 新しいタブか）。変更はそのつど保存する。
pub(super) fn manage(owner: HWND) -> Option<(String, bool)> {
    let mut t = Template::dialog("ブックマークの管理", 420, 250);
    t.item(
        (WS_BORDER | WS_TABSTOP | WS_VSCROLL | WS_HSCROLL).0
            | (LBS_NOTIFY | LBS_NOINTEGRALHEIGHT) as u32,
        7,
        7,
        334,
        214,
        M_LIST,
        CLASS_LISTBOX,
        "",
    );
    let mut y = 7;
    for (id, text) in [
        (M_OPEN, "開く"),
        (M_OPEN_TAB, "新しいタブで開く"),
        (M_EDIT, "編集..."),
        (M_DELETE, "削除"),
    ] {
        t.item(
            WS_TABSTOP.0 | BS_PUSHBUTTON as u32,
            347,
            y,
            66,
            14,
            id,
            CLASS_BUTTON,
            text,
        );
        y += 17;
    }
    t.item(
        WS_TABSTOP.0 | BS_PUSHBUTTON as u32,
        347,
        y + 4,
        32,
        14,
        M_UP,
        CLASS_BUTTON,
        "↑",
    );
    t.item(
        WS_TABSTOP.0 | BS_PUSHBUTTON as u32,
        381,
        y + 4,
        32,
        14,
        M_DOWN,
        CLASS_BUTTON,
        "↓",
    );
    t.item(
        WS_TABSTOP.0 | BS_PUSHBUTTON as u32,
        347,
        160,
        66,
        14,
        M_IMPORT,
        CLASS_BUTTON,
        "読み込み...",
    );
    t.item(
        WS_TABSTOP.0 | BS_PUSHBUTTON as u32,
        347,
        177,
        66,
        14,
        M_EXPORT,
        CLASS_BUTTON,
        "書き出し...",
    );
    t.item(0, 7, 230, 260, 10, M_COUNT, CLASS_STATIC, "");
    t.item(
        WS_TABSTOP.0 | BS_DEFPUSHBUTTON as u32,
        363,
        228,
        50,
        14,
        IDCANCEL_,
        CLASS_BUTTON,
        "閉じる",
    );
    let mut st = ManageState {
        list: load(),
        open: None,
    };
    run(owner, t, Some(manage_proc), &mut st);
    st.open
}

fn manage_fill(dlg: HWND, st: &ManageState, sel: Option<usize>) {
    unsafe {
        SendDlgItemMessageW(dlg, M_LIST as i32, LB_RESETCONTENT, WPARAM(0), LPARAM(0));
        let mut widest = 0usize;
        for b in &st.list.items {
            let s = if b.folder.is_empty() {
                format!("{}　—　{}", b.label(), b.url)
            } else {
                format!("📁 {} ／ {}　—　{}", b.folder, b.label(), b.url)
            };
            widest = widest.max(s.chars().count());
            let s = HSTRING::from(s);
            SendDlgItemMessageW(
                dlg,
                M_LIST as i32,
                LB_ADDSTRING,
                WPARAM(0),
                LPARAM(s.as_ptr() as isize),
            );
        }
        // 長い行は横に送って見られるように
        SendDlgItemMessageW(
            dlg,
            M_LIST as i32,
            LB_SETHORIZONTALEXTENT,
            WPARAM(widest * 14),
            LPARAM(0),
        );
        if let Some(i) = sel.filter(|i| *i < st.list.items.len()) {
            SendDlgItemMessageW(dlg, M_LIST as i32, LB_SETCURSEL, WPARAM(i), LPARAM(0));
        }
    }
    set_text(
        dlg,
        M_COUNT,
        &format!("{} 件（{}）", st.list.items.len(), path().display()),
    );
}

fn manage_sel(dlg: HWND) -> Option<usize> {
    let i =
        unsafe { SendDlgItemMessageW(dlg, M_LIST as i32, LB_GETCURSEL, WPARAM(0), LPARAM(0)).0 };
    (i >= 0).then_some(i as usize)
}

/// 読み直してから `f` で変え、保存して出し直す。
fn manage_change(dlg: HWND, st: &mut ManageState, f: impl FnOnce(&mut Bookmarks) -> Option<usize>) {
    st.list = load();
    let sel = f(&mut st.list);
    save(dlg, &st.list);
    manage_fill(dlg, st, sel);
}

extern "system" fn manage_proc(dlg: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> isize {
    match msg {
        WM_INITDIALOG => {
            unsafe { SetWindowLongPtrW(dlg, GWLP_USERDATA, lparam.0) };
            let st = state_of::<ManageState>(dlg);
            manage_fill(dlg, st, Some(0));
            1
        }
        WM_COMMAND => {
            let st = state_of::<ManageState>(dlg);
            let id = (wparam.0 & 0xffff) as u16;
            let code = ((wparam.0 >> 16) & 0xffff) as u32;
            match id {
                M_OPEN | M_OPEN_TAB => {
                    if let Some(b) = manage_sel(dlg).and_then(|i| st.list.items.get(i)) {
                        st.open = Some((b.url.clone(), id == M_OPEN_TAB));
                        end(dlg, IDOK_);
                    }
                }
                M_LIST if code == LBN_DBLCLK => {
                    if let Some(b) = manage_sel(dlg).and_then(|i| st.list.items.get(i)) {
                        st.open = Some((b.url.clone(), false));
                        end(dlg, IDOK_);
                    }
                }
                M_EDIT => {
                    let Some(i) = manage_sel(dlg) else { return 1 };
                    let Some(old) = st.list.items.get(i).cloned() else {
                        return 1;
                    };
                    match edit(dlg, old.clone(), true, st.list.folders()) {
                        EditResult::Save(b) => manage_change(dlg, st, |l| {
                            let i = l.find(&old.url)?;
                            l.items[i] = b;
                            Some(i)
                        }),
                        EditResult::Delete => manage_change(dlg, st, |l| {
                            let i = l.find(&old.url)?;
                            l.items.remove(i);
                            Some(i)
                        }),
                        EditResult::Cancel => {}
                    }
                }
                M_DELETE => {
                    let Some(url) = manage_sel(dlg)
                        .and_then(|i| st.list.items.get(i))
                        .map(|b| b.url.clone())
                    else {
                        return 1;
                    };
                    manage_change(dlg, st, |l| {
                        let i = l.find(&url)?;
                        l.items.remove(i);
                        Some(i.min(l.items.len().saturating_sub(1)))
                    });
                }
                M_UP | M_DOWN => {
                    let Some(url) = manage_sel(dlg)
                        .and_then(|i| st.list.items.get(i))
                        .map(|b| b.url.clone())
                    else {
                        return 1;
                    };
                    manage_change(dlg, st, |l| {
                        let i = l.find(&url)?;
                        l.shift(i, id == M_UP).or(Some(i))
                    });
                }
                M_IMPORT => {
                    let Some(file) = crate::fm::pick_file(
                        dlg,
                        ("ブックマークの HTML (*.html;*.htm)", "*.html;*.htm"),
                        None,
                        None,
                    ) else {
                        return 1;
                    };
                    match std::fs::read(&file) {
                        Ok(bytes) => {
                            let html = String::from_utf8_lossy(&bytes);
                            let mut added = 0;
                            manage_change(dlg, st, |l| {
                                added = l.import_html(&html);
                                Some(l.items.len().saturating_sub(1))
                            });
                            crate::util::info_box(
                                dlg,
                                &format!(
                                    "{added} 件を足しました（同じ URL のものは足していません）。"
                                ),
                            );
                        }
                        Err(e) => crate::util::error_box(
                            dlg,
                            &format!("読めません: {}: {e}", file.display()),
                        ),
                    }
                }
                M_EXPORT => {
                    let Some(file) = crate::fm::pick_file(
                        dlg,
                        ("ブックマークの HTML (*.html)", "*.html"),
                        None,
                        Some("bookmarks.html"),
                    ) else {
                        return 1;
                    };
                    match std::fs::write(&file, load().export_html()) {
                        Ok(()) => crate::util::info_box(
                            dlg,
                            &format!(
                                "書き出しました: {}\nEdge・Chrome・Firefox の「ブックマークのインポート（HTML）」で読み込めます。",
                                file.display()
                            ),
                        ),
                        Err(e) => crate::util::error_box(
                            dlg,
                            &format!("書き出せません: {}: {e}", file.display()),
                        ),
                    }
                }
                IDCANCEL_ => end(dlg, IDCANCEL_),
                _ => {}
            }
            1
        }
        _ => 0,
    }
}
