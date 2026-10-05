//! 「最近開いたファイル」（履歴）と「ブックマーク」のダイアログ。
//!
//! 一覧は開くたびに設定フォルダのファイルから読み、削除・並べ替え・ブックマークへの追加は
//! その場でファイルに書く（[`yy_config::recent::PathList::update`]）。

use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::HSTRING;
use yy_config::recent::{self, PathList};

use crate::goto::{CLASS_BUTTON, CLASS_EDIT, CLASS_STATIC, Template};

const ID_FILTER: u16 = 100;
const ID_LIST: u16 = 101;
const ID_REMOVE: u16 = 102;
const ID_BOOKMARK: u16 = 103;
const ID_UP: u16 = 104;
const ID_DOWN: u16 = 105;
const ID_INFO: u16 = 106;
const IDOK_: u16 = 1;
const IDCANCEL_: u16 = 2;
/// リストボックスのクラス（定義済みのクラスの番号）
const CLASS_LISTBOX: u16 = 0x0083;

/// どちらの一覧か。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ListKind {
    History,
    Bookmarks,
}

impl ListKind {
    fn file_name(self) -> &'static str {
        match self {
            ListKind::History => recent::HISTORY_FILE,
            ListKind::Bookmarks => recent::BOOKMARK_FILE,
        }
    }

    pub(crate) fn limit(self) -> usize {
        match self {
            ListKind::History => recent::HISTORY_LIMIT,
            ListKind::Bookmarks => recent::BOOKMARK_LIMIT,
        }
    }

    fn title(self) -> &'static str {
        match self {
            ListKind::History => "最近開いたファイル",
            ListKind::Bookmarks => "ブックマーク",
        }
    }

    /// 保存先のファイル（設定フォルダがなければ `None`）。
    pub(crate) fn file(self) -> Option<PathBuf> {
        recent::list_file(self.file_name())
    }

    /// 保存してある一覧。
    pub(crate) fn load(self) -> PathList {
        match self.file() {
            Some(f) => PathList::load(&f, self.limit()),
            None => PathList::new(self.limit()),
        }
    }

    /// 一覧を読み直して `f` で変更し、保存する。
    pub(crate) fn update<R>(self, f: impl FnOnce(&mut PathList) -> R) -> Option<R> {
        let file = self.file()?;
        match PathList::update(&file, self.limit(), f) {
            Ok((_, r)) => Some(r),
            Err(e) => {
                eprintln!("{} を保存できません: {e}", file.display());
                None
            }
        }
    }
}

/// 一覧に記録する形（絶対パス。Windows では区切りを `\` にそろえる）。
/// SSH 接続先のファイル（`ssh://…`）はそのまま。
pub(crate) fn normalize(path: &Path) -> PathBuf {
    if recent::is_remote(path) {
        return path.to_owned();
    }
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_owned());
    PathBuf::from(abs.to_string_lossy().replace('/', "\\"))
}

/// ファイルを開いたことを履歴に記録する。
pub(crate) fn remember(path: &Path) {
    let path = normalize(path);
    ListKind::History.update(|l| l.touch(&path));
}

/// ブックマークに加える。既にあれば `Ok(false)`、上限に達していれば `Err(())`。
pub(crate) fn add_bookmark(path: &Path) -> Result<bool, ()> {
    let path = normalize(path);
    ListKind::Bookmarks
        .update(|l| l.push(&path))
        .unwrap_or(Ok(false))
}

/// ダイアログとの間で受け渡す値。
struct State {
    kind: ListKind,
    list: PathList,
    /// リストボックスの各行が指す `list` の番号
    shown: Vec<usize>,
    /// 開くファイル（OK で閉じたとき）
    open: Vec<PathBuf>,
}

fn build_template(kind: ListKind) -> Template {
    let (w, h) = (360i16, 230i16);
    let mut t = Template::dialog(kind.title(), w, h);
    let button = WS_TABSTOP.0 | BS_PUSHBUTTON as u32;
    t.item(0, 7, 9, 40, 10, 0, CLASS_STATIC, "絞り込み:");
    t.item(
        (WS_BORDER | WS_TABSTOP).0 | ES_AUTOHSCROLL as u32,
        48,
        7,
        w - 55,
        13,
        ID_FILTER,
        CLASS_EDIT,
        "",
    );
    let list_style = (WS_BORDER | WS_VSCROLL | WS_HSCROLL | WS_TABSTOP).0
        | (LBS_NOTIFY | LBS_EXTENDEDSEL | LBS_NOINTEGRALHEIGHT | LBS_USETABSTOPS) as u32;
    t.item(
        list_style,
        7,
        26,
        w - 14,
        h - 66,
        ID_LIST,
        CLASS_LISTBOX,
        "",
    );
    t.item(0, 7, h - 36, w - 14, 10, ID_INFO, CLASS_STATIC, "");
    let y = h - 21;
    t.item(
        WS_TABSTOP.0 | BS_DEFPUSHBUTTON as u32,
        7,
        y,
        50,
        14,
        IDOK_,
        CLASS_BUTTON,
        "開く",
    );
    t.item(
        button,
        61,
        y,
        50,
        14,
        ID_REMOVE,
        CLASS_BUTTON,
        "一覧から削除",
    );
    match kind {
        ListKind::History => {
            t.item(
                button,
                115,
                y,
                80,
                14,
                ID_BOOKMARK,
                CLASS_BUTTON,
                "ブックマークに追加",
            );
        }
        ListKind::Bookmarks => {
            t.item(button, 115, y, 40, 14, ID_UP, CLASS_BUTTON, "上へ");
            t.item(button, 159, y, 40, 14, ID_DOWN, CLASS_BUTTON, "下へ");
        }
    }
    t.item(button, w - 57, y, 50, 14, IDCANCEL_, CLASS_BUTTON, "閉じる");
    t
}

/// 一覧のダイアログを表示し、開くファイルを返す（なければ空）。
pub(crate) fn show(owner: HWND, kind: ListKind) -> Vec<PathBuf> {
    if kind.file().is_none() {
        return Vec::new();
    }
    let aligned = build_template(kind).aligned();
    let mut state = State {
        kind,
        list: kind.load(),
        shown: Vec::new(),
        open: Vec::new(),
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
    if r == IDOK_ as isize {
        state.open
    } else {
        Vec::new()
    }
}

/// 行の表示（ファイル名とフォルダ）。
fn row_text(p: &Path) -> String {
    let name = p.file_name().map_or_else(
        || p.to_string_lossy().into_owned(),
        |n| n.to_string_lossy().into_owned(),
    );
    let dir = p
        .parent()
        .map(|d| d.to_string_lossy().into_owned())
        .unwrap_or_default();
    format!("{name}\t{dir}")
}

/// 絞り込みの語（空白区切り。すべてを含むパスを残す。大文字・小文字は区別しない）。
pub(crate) fn matches(path: &Path, filter: &str) -> bool {
    let p = path.to_string_lossy().to_lowercase();
    filter
        .split_whitespace()
        .all(|w| p.contains(&w.to_lowercase()))
}

unsafe fn state<'a>(hwnd: HWND) -> &'a mut State {
    unsafe { &mut *(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State) }
}

fn item(hwnd: HWND, id: u16) -> HWND {
    unsafe { GetDlgItem(Some(hwnd), id as i32).unwrap_or_default() }
}

fn filter_text(hwnd: HWND) -> String {
    let mut buf = [0u16; 512];
    let n = unsafe { GetDlgItemTextW(hwnd, ID_FILTER as i32, &mut buf) } as usize;
    String::from_utf16_lossy(&buf[..n])
}

/// 選択している行の `list` の番号。
fn selected(hwnd: HWND) -> Vec<usize> {
    let st = unsafe { state(hwnd) };
    let lb = item(hwnd, ID_LIST);
    let n = unsafe { SendMessageW(lb, LB_GETSELCOUNT, None, None).0 };
    if n <= 0 {
        return Vec::new();
    }
    let mut rows = vec![0i32; n as usize];
    unsafe {
        SendMessageW(
            lb,
            LB_GETSELITEMS,
            Some(WPARAM(n as usize)),
            Some(LPARAM(rows.as_mut_ptr() as isize)),
        );
    }
    rows.iter()
        .filter_map(|&r| st.shown.get(r as usize).copied())
        .collect()
}

/// 一覧を表示し直す。`select` の項目（`list` の番号）を選ぶ。
fn refill(hwnd: HWND, select: &[usize]) {
    let st = unsafe { state(hwnd) };
    let lb = item(hwnd, ID_LIST);
    let filter = filter_text(hwnd);
    unsafe {
        SendMessageW(lb, WM_SETREDRAW, Some(WPARAM(0)), None);
        SendMessageW(lb, LB_RESETCONTENT, None, None);
    }
    st.shown.clear();
    let mut widest = 0;
    for (i, p) in st.list.items().iter().enumerate() {
        if !matches(p, &filter) {
            continue;
        }
        let text = row_text(p);
        widest = widest.max(text.chars().count());
        let row = unsafe {
            SendMessageW(
                lb,
                LB_ADDSTRING,
                None,
                Some(LPARAM(HSTRING::from(text).as_ptr() as isize)),
            )
            .0
        };
        if select.contains(&i) {
            unsafe {
                SendMessageW(lb, LB_SETSEL, Some(WPARAM(1)), Some(LPARAM(row)));
            }
        }
        st.shown.push(i);
    }
    unsafe {
        // 横スクロールできる幅（おおよそ）
        SendMessageW(lb, LB_SETHORIZONTALEXTENT, Some(WPARAM(widest * 9)), None);
        if select.is_empty() && !st.shown.is_empty() {
            SendMessageW(lb, LB_SETSEL, Some(WPARAM(1)), Some(LPARAM(0)));
        }
        SendMessageW(lb, WM_SETREDRAW, Some(WPARAM(1)), None);
        let _ = windows::Win32::Graphics::Gdi::InvalidateRect(Some(lb), None, true);
    }
    let info = if st.shown.len() == st.list.len() {
        format!("{} 件（上限 {} 件）", st.list.len(), st.list.limit())
    } else {
        format!(
            "{} 件中 {} 件（上限 {} 件）",
            st.list.len(),
            st.shown.len(),
            st.list.limit()
        )
    };
    set_info(hwnd, &info);
}

fn set_info(hwnd: HWND, text: &str) {
    unsafe {
        let _ = SetDlgItemTextW(hwnd, ID_INFO as i32, &HSTRING::from(text));
    }
}

/// 選択している項目を一覧から取り除く。
fn remove_selected(hwnd: HWND) {
    let st = unsafe { state(hwnd) };
    let paths: Vec<PathBuf> = selected(hwnd)
        .into_iter()
        .map(|i| st.list.items()[i].clone())
        .collect();
    if paths.is_empty() {
        return;
    }
    let kind = st.kind;
    kind.update(|l| {
        for p in &paths {
            l.remove(p);
        }
    });
    st.list = kind.load();
    refill(hwnd, &[]);
}

/// 選択している項目（1 つ）を前後に動かす（ブックマーク）。
fn shift_selected(hwnd: HWND, up: bool) {
    let st = unsafe { state(hwnd) };
    let sel = selected(hwnd);
    let [i] = sel[..] else {
        return;
    };
    if !filter_text(hwnd).trim().is_empty() {
        set_info(hwnd, "絞り込みを消してから並べ替えてください");
        return;
    }
    let p = st.list.items()[i].clone();
    st.kind.update(|l| l.shift(&p, up));
    st.list = st.kind.load();
    let at = st.list.items().iter().position(|q| *q == p);
    refill(hwnd, &at.into_iter().collect::<Vec<_>>());
}

/// 選択している項目をブックマークに加える（履歴）。
fn bookmark_selected(hwnd: HWND) {
    let st = unsafe { state(hwnd) };
    let mut added = 0;
    for i in selected(hwnd) {
        match add_bookmark(&st.list.items()[i]) {
            Ok(true) => added += 1,
            Ok(false) => {}
            Err(()) => {
                set_info(
                    hwnd,
                    &format!(
                        "ブックマークは {} 件までです（{added} 件追加しました）",
                        recent::BOOKMARK_LIMIT
                    ),
                );
                return;
            }
        }
    }
    set_info(hwnd, &format!("ブックマークに {added} 件追加しました"));
}

extern "system" fn dialog_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> isize {
    unsafe {
        match msg {
            WM_INITDIALOG => {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, lparam.0);
                // ファイル名の列とフォルダの列（ダイアログ単位）
                let stops = [120i32];
                SendMessageW(
                    item(hwnd, ID_LIST),
                    LB_SETTABSTOPS,
                    Some(WPARAM(1)),
                    Some(LPARAM(stops.as_ptr() as isize)),
                );
                refill(hwnd, &[]);
                1
            }
            WM_COMMAND => {
                let id = crate::loword(wparam.0) as u16;
                let code = crate::hiword(wparam.0);
                match id {
                    ID_FILTER if code == EN_CHANGE => refill(hwnd, &[]),
                    ID_LIST if code == LBN_DBLCLK => open_selected(hwnd),
                    IDOK_ => open_selected(hwnd),
                    IDCANCEL_ => {
                        let _ = EndDialog(hwnd, IDCANCEL_ as isize);
                    }
                    ID_REMOVE => remove_selected(hwnd),
                    ID_BOOKMARK => bookmark_selected(hwnd),
                    ID_UP => shift_selected(hwnd, true),
                    ID_DOWN => shift_selected(hwnd, false),
                    _ => return 0,
                }
                1
            }
            _ => 0,
        }
    }
}

fn open_selected(hwnd: HWND) {
    let st = unsafe { state(hwnd) };
    let sel = selected(hwnd);
    if sel.is_empty() {
        return;
    }
    st.open = sel.iter().map(|&i| st.list.items()[i].clone()).collect();
    unsafe {
        let _ = EndDialog(hwnd, IDOK_ as isize);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_by_all_words_ignoring_case() {
        let p = Path::new("C:\\Work\\Report\\Sales 2026.csv");
        assert!(matches(p, ""));
        assert!(matches(p, "sales"));
        assert!(matches(p, "work  CSV"));
        assert!(!matches(p, "work txt"));
        assert_eq!(row_text(p), "Sales 2026.csv\tC:\\Work\\Report");
        // 区切りの / は \ にそろえる
        assert_eq!(
            normalize(Path::new("C:/Work/a.txt")),
            PathBuf::from("C:\\Work\\a.txt")
        );
    }
}
