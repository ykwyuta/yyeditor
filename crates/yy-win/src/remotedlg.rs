//! リモートのファイルを選ぶダイアログ（11 章 7.2）。
//!
//! Windows のファイルダイアログは SSH の接続先を参照できないため、接続先・フォルダ・一覧・
//! ファイル名・文字コードを持つ独自のダイアログで選ぶ。一覧はエージェントから取得する。

use std::sync::Arc;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::GetFocus;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::HSTRING;
use yy_core::Encoding;
use yy_remote::uri::{RemoteUri, Target};
use yy_remote::{DirEntry, FileInfo, Session};

use crate::goto::{CLASS_BUTTON, CLASS_EDIT, CLASS_STATIC, Template};
use crate::remote::{self, RemoteDest};
use crate::util::human_size;

const ID_HOST: u16 = 100;
const ID_CONNECT: u16 = 101;
const ID_DIR: u16 = 102;
const ID_LIST: u16 = 103;
const ID_NAME: u16 = 104;
const ID_ENCODING: u16 = 105;
const ID_BOM: u16 = 106;
const ID_INFO: u16 = 107;
const IDOK_: u16 = 1;
const IDCANCEL_: u16 = 2;
const CLASS_LISTBOX: u16 = 0x0083;
const CLASS_COMBOBOX: u16 = 0x0085;

/// 開くか保存するか。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    Open,
    Save,
}

/// 選んだファイル。
pub(crate) struct Picked {
    pub(crate) uri: RemoteUri,
    pub(crate) session: Arc<Session>,
    /// 開く: 指定した文字コード（`None` なら自動判別）。保存: 保存する文字コード
    pub(crate) encoding: Option<Encoding>,
    pub(crate) bom: bool,
    /// 選んだファイルが既にあれば、その情報
    pub(crate) existing: Option<FileInfo>,
}

impl Picked {
    /// 保存先（既にあるファイルは上書きを確かめてあるので置き換える）。
    pub(crate) fn dest(&self) -> RemoteDest {
        RemoteDest {
            uri: self.uri.clone(),
            session: self.session.clone(),
            expected: self.existing.as_ref().map(|i| i.id),
            force: self.existing.is_some(),
        }
    }
}

struct State {
    mode: Mode,
    targets: Vec<String>,
    target: Option<Target>,
    session: Option<Arc<Session>>,
    /// 表示しているフォルダ（接続前は初期値）
    dir: Vec<u8>,
    /// 一覧の各行の項目（先頭の「..」は `None`）
    shown: Vec<Option<DirEntry>>,
    name: String,
    encoding: Option<Encoding>,
    bom: bool,
    result: Option<Picked>,
}

/// ダイアログを表示する。`initial` は初期値の場所（ファイルまたはフォルダ）。
pub(crate) fn show(
    owner: HWND,
    mode: Mode,
    initial: Option<RemoteUri>,
    encoding: Option<Encoding>,
    bom: bool,
) -> Option<Picked> {
    let targets = crate::app::with_app(|a| a.remote.known_targets()).unwrap_or_default();
    let (target, dir, name) = match &initial {
        Some(u) => {
            let is_file = mode == Mode::Save || !u.path.ends_with(b"/");
            if is_file {
                (
                    Some(u.target()),
                    yy_proto::parent_path(&u.path),
                    yy_proto::display_path(yy_proto::file_name(&u.path)),
                )
            } else {
                (Some(u.target()), u.path.clone(), String::new())
            }
        }
        None => (None, Vec::new(), String::new()),
    };
    let aligned = build_template(mode).aligned();
    let mut state = State {
        mode,
        targets,
        target,
        session: None,
        dir,
        shown: Vec::new(),
        name,
        encoding,
        bom,
        result: None,
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
        state.result
    } else {
        None
    }
}

fn build_template(mode: Mode) -> Template {
    let (w, h) = (400i16, 282i16);
    let count = if mode == Mode::Save { 15 } else { 14 };
    let title = match mode {
        Mode::Open => "リモートのファイルを開く",
        Mode::Save => "リモートに名前を付けて保存",
    };
    let mut t = Template::dialog(title, count, w, h);
    let tab = WS_TABSTOP.0;
    t.item(0, 7, 9, 42, 10, 0, CLASS_STATIC, "接続先:");
    t.item(
        tab | WS_VSCROLL.0 | (CBS_DROPDOWN | CBS_AUTOHSCROLL) as u32,
        52,
        7,
        w - 52 - 74,
        120,
        ID_HOST,
        CLASS_COMBOBOX,
        "",
    );
    t.item(
        tab | BS_PUSHBUTTON as u32,
        w - 67,
        6,
        60,
        14,
        ID_CONNECT,
        CLASS_BUTTON,
        "接続(&C)",
    );
    t.item(0, 7, 28, 42, 10, 0, CLASS_STATIC, "フォルダ:");
    t.item(
        (WS_BORDER | WS_TABSTOP).0 | ES_AUTOHSCROLL as u32,
        52,
        26,
        w - 59,
        13,
        ID_DIR,
        CLASS_EDIT,
        "",
    );
    let list_style = (WS_BORDER | WS_VSCROLL | WS_HSCROLL | WS_TABSTOP).0
        | (LBS_NOTIFY | LBS_NOINTEGRALHEIGHT | LBS_USETABSTOPS) as u32;
    t.item(list_style, 7, 45, w - 14, 150, ID_LIST, CLASS_LISTBOX, "");
    t.item(0, 7, 203, 42, 10, 0, CLASS_STATIC, "ファイル名:");
    t.item(
        (WS_BORDER | WS_TABSTOP).0 | ES_AUTOHSCROLL as u32,
        52,
        201,
        w - 59,
        13,
        ID_NAME,
        CLASS_EDIT,
        "",
    );
    t.item(0, 7, 222, 42, 10, 0, CLASS_STATIC, "文字コード:");
    t.item(
        tab | WS_VSCROLL.0 | CBS_DROPDOWNLIST as u32,
        52,
        220,
        150,
        200,
        ID_ENCODING,
        CLASS_COMBOBOX,
        "",
    );
    if mode == Mode::Save {
        t.item(
            tab | BS_AUTOCHECKBOX as u32,
            210,
            221,
            80,
            12,
            ID_BOM,
            CLASS_BUTTON,
            "BOM を付ける",
        );
    }
    t.item(0, 7, 240, w - 14, 18, ID_INFO, CLASS_STATIC, "");
    let ok = match mode {
        Mode::Open => "開く",
        Mode::Save => "保存",
    };
    t.item(
        tab | BS_DEFPUSHBUTTON as u32,
        w - 111,
        h - 21,
        50,
        14,
        IDOK_,
        CLASS_BUTTON,
        ok,
    );
    t.item(
        tab | BS_PUSHBUTTON as u32,
        w - 57,
        h - 21,
        50,
        14,
        IDCANCEL_,
        CLASS_BUTTON,
        "キャンセル",
    );
    t
}

unsafe fn state<'a>(hwnd: HWND) -> &'a mut State {
    unsafe { &mut *(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State) }
}

fn item(hwnd: HWND, id: u16) -> HWND {
    unsafe { GetDlgItem(Some(hwnd), id as i32).unwrap_or_default() }
}

fn text_of(hwnd: HWND, id: u16) -> String {
    let mut buf = vec![0u16; 4096];
    let n = unsafe { GetDlgItemTextW(hwnd, id as i32, &mut buf) } as usize;
    String::from_utf16_lossy(&buf[..n])
}

fn set_text(hwnd: HWND, id: u16, text: &str) {
    unsafe {
        let _ = SetDlgItemTextW(hwnd, id as i32, &HSTRING::from(text));
    }
}

fn set_info(hwnd: HWND, text: &str) {
    set_text(hwnd, ID_INFO, text);
}

/// 文字コードの欄の項目（開く: 先頭が「自動判別」）。
fn encoding_items(mode: Mode) -> Vec<Option<Encoding>> {
    let mut v: Vec<Option<Encoding>> = Vec::new();
    if mode == Mode::Open {
        v.push(None);
    }
    v.extend(Encoding::all().iter().copied().map(Some));
    v
}

fn fill_encodings(hwnd: HWND) {
    let st = unsafe { state(hwnd) };
    let combo = item(hwnd, ID_ENCODING);
    let mut selected = 0;
    for (i, e) in encoding_items(st.mode).iter().enumerate() {
        let label = match e {
            None => "自動判別".to_owned(),
            Some(e) => e.label().to_owned(),
        };
        unsafe {
            SendMessageW(
                combo,
                CB_ADDSTRING,
                None,
                Some(LPARAM(HSTRING::from(label).as_ptr() as isize)),
            );
        }
        if *e == st.encoding {
            selected = i;
        }
    }
    unsafe {
        SendMessageW(combo, CB_SETCURSEL, Some(WPARAM(selected)), None);
        if st.mode == Mode::Save && st.bom {
            SendMessageW(item(hwnd, ID_BOM), BM_SETCHECK, Some(WPARAM(1)), None);
        }
    }
}

fn selected_encoding(hwnd: HWND) -> Option<Encoding> {
    let st = unsafe { state(hwnd) };
    let i = unsafe { SendMessageW(item(hwnd, ID_ENCODING), CB_GETCURSEL, None, None).0 };
    encoding_items(st.mode)
        .get(i.max(0) as usize)
        .copied()
        .flatten()
}

/// 接続先の欄の内容で接続し、フォルダを表示する。
fn connect(hwnd: HWND) {
    let st = unsafe { state(hwnd) };
    let text = text_of(hwnd, ID_HOST);
    let Some(target) = Target::parse(&text) else {
        set_info(
            hwnd,
            "接続先を「ユーザー@ホスト:ポート」または設定の名前で入力してください",
        );
        return;
    };
    let show = |t: &str| set_info(hwnd, t);
    match remote::session(&target, &show) {
        Ok(s) => {
            // 前とは別の接続先なら、フォルダは接続先のホームから
            let same = st.target.as_ref().is_some_and(|t| t.same(&target));
            if !same || st.dir.is_empty() {
                st.dir = s.home().to_vec();
            }
            st.target = Some(target);
            st.session = Some(s);
            let dir = st.dir.clone();
            list(hwnd, &dir);
        }
        Err(e) => {
            set_info(hwnd, "");
            crate::util::error_box(hwnd, &e);
        }
    }
}

/// フォルダ `dir`（`~` も可）を表示する。
fn list(hwnd: HWND, dir: &[u8]) {
    let st = unsafe { state(hwnd) };
    let Some(session) = st.session.clone() else {
        set_info(hwnd, "先に接続してください");
        return;
    };
    let dir = dir.to_vec();
    let show = |t: &str| set_info(hwnd, t);
    set_info(hwnd, "一覧を取得しています…");
    let r = remote::wait(&show, move |_| {
        let real = session.real_path(&dir)?;
        let entries = session.read_dir(&real)?;
        Ok::<_, std::io::Error>((real, entries))
    });
    let (real, mut entries) = match r {
        Ok(v) => v,
        Err(e) => {
            set_info(hwnd, &format!("フォルダを開けません: {e}"));
            return;
        }
    };
    // フォルダを先に、それぞれ名前順
    entries.sort_by(|a, b| {
        let da = a.info.as_ref().is_some_and(|i| i.is_dir());
        let db = b.info.as_ref().is_some_and(|i| i.is_dir());
        db.cmp(&da).then_with(|| a.name.cmp(&b.name))
    });
    st.dir = real;
    set_text(hwnd, ID_DIR, &yy_proto::display_path(&st.dir));
    st.shown.clear();
    let lb = item(hwnd, ID_LIST);
    unsafe {
        SendMessageW(lb, WM_SETREDRAW, Some(WPARAM(0)), None);
        SendMessageW(lb, LB_RESETCONTENT, None, None);
    }
    let add = |text: String| unsafe {
        SendMessageW(
            lb,
            LB_ADDSTRING,
            None,
            Some(LPARAM(HSTRING::from(text).as_ptr() as isize)),
        );
    };
    if st.dir != b"/" {
        add("..".into());
        st.shown.push(None);
    }
    let count = entries.len();
    for e in entries {
        let name = yy_proto::display_path(&e.name);
        let text = match &e.info {
            Some(i) if i.is_dir() => format!("{name}/"),
            Some(i) => format!("{name}\t{}", human_size(i.len())),
            None => format!("{name}\t（リンク切れ）"),
        };
        add(text);
        st.shown.push(Some(e));
    }
    unsafe {
        SendMessageW(lb, LB_SETHORIZONTALEXTENT, Some(WPARAM(2000)), None);
        SendMessageW(lb, WM_SETREDRAW, Some(WPARAM(1)), None);
        let _ = windows::Win32::Graphics::Gdi::InvalidateRect(Some(lb), None, true);
    }
    set_info(hwnd, &format!("{count} 項目"));
}

/// 一覧で選んでいる行。
fn selected_row(hwnd: HWND) -> Option<Option<DirEntry>> {
    let st = unsafe { state(hwnd) };
    let i = unsafe { SendMessageW(item(hwnd, ID_LIST), LB_GETCURSEL, None, None).0 };
    if i < 0 {
        return None;
    }
    st.shown.get(i as usize).cloned()
}

/// 一覧の行を開く（フォルダなら移動、ファイルなら選ぶ）。
fn activate_row(hwnd: HWND) {
    let st = unsafe { state(hwnd) };
    match selected_row(hwnd) {
        Some(None) => {
            let up = yy_proto::parent_path(&st.dir);
            list(hwnd, &up);
        }
        Some(Some(e)) if e.info.as_ref().is_some_and(|i| i.is_dir()) => {
            let next = yy_proto::join_path(&st.dir, &e.name);
            list(hwnd, &next);
        }
        Some(Some(e)) => {
            set_text(hwnd, ID_NAME, &yy_proto::display_path(&e.name));
            accept_name(hwnd);
        }
        None => {}
    }
}

/// フォルダの欄に入力した場所へ移動する（ファイルならそれを選ぶ）。
fn go_to_dir_field(hwnd: HWND) {
    let st = unsafe { state(hwnd) };
    let typed = text_of(hwnd, ID_DIR);
    let typed = typed.trim();
    if typed.is_empty() {
        return;
    }
    let Some(session) = st.session.clone() else {
        connect(hwnd);
        return;
    };
    let path = session.expand_home(typed.as_bytes());
    let p = path.clone();
    let show = |t: &str| set_info(hwnd, t);
    match remote::wait(&show, move |_| session.stat(&p)) {
        Ok(i) if i.is_dir() => list(hwnd, &path),
        Ok(_) => {
            st.dir = yy_proto::parent_path(&path);
            set_text(
                hwnd,
                ID_NAME,
                &yy_proto::display_path(yy_proto::file_name(&path)),
            );
            let dir = st.dir.clone();
            list(hwnd, &dir);
            accept_name(hwnd);
        }
        Err(e) => set_info(hwnd, &format!("{typed}: {e}")),
    }
}

/// ファイル名の欄のファイルに決める。
fn accept_name(hwnd: HWND) {
    let st = unsafe { state(hwnd) };
    let (Some(session), Some(target)) = (st.session.clone(), st.target.clone()) else {
        connect(hwnd);
        return;
    };
    let name = text_of(hwnd, ID_NAME);
    let name = name.trim();
    if name.is_empty() {
        set_info(hwnd, "ファイルを選ぶか、ファイル名を入力してください");
        return;
    }
    let path = if name.starts_with('~') {
        session.expand_home(name.as_bytes())
    } else {
        yy_proto::join_path(&st.dir, name.as_bytes())
    };
    let p = path.clone();
    let s = session.clone();
    let show = |t: &str| set_info(hwnd, t);
    let existing = match remote::wait(&show, move |_| s.stat(&p)) {
        Ok(i) if i.is_dir() => {
            set_text(hwnd, ID_NAME, "");
            list(hwnd, &path);
            return;
        }
        Ok(i) => Some(i),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            set_info(hwnd, &format!("{name}: {e}"));
            return;
        }
    };
    let uri = RemoteUri {
        user: target.user.clone(),
        host: target.host.clone(),
        port: target.port,
        path,
    };
    match (st.mode, &existing) {
        (Mode::Open, None) => {
            set_info(hwnd, &format!("{name} が見つかりません"));
            return;
        }
        (Mode::Save, Some(_)) => {
            let text = format!("{uri}\n\nは既にあります。置き換えますか？");
            let r = unsafe {
                MessageBoxW(
                    Some(hwnd),
                    &HSTRING::from(text),
                    &HSTRING::from("yyeditor"),
                    MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2,
                )
            };
            if r != IDYES {
                return;
            }
        }
        _ => {}
    }
    let bom = st.mode == Mode::Save
        && unsafe { SendMessageW(item(hwnd, ID_BOM), BM_GETCHECK, None, None).0 } == 1;
    st.result = Some(Picked {
        uri,
        session,
        encoding: selected_encoding(hwnd),
        bom,
        existing,
    });
    unsafe {
        let _ = EndDialog(hwnd, IDOK_ as isize);
    }
}

extern "system" fn dialog_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> isize {
    unsafe {
        match msg {
            WM_INITDIALOG => {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, lparam.0);
                let st = state(hwnd);
                let combo = item(hwnd, ID_HOST);
                for t in &st.targets {
                    SendMessageW(
                        combo,
                        CB_ADDSTRING,
                        None,
                        Some(LPARAM(HSTRING::from(t.as_str()).as_ptr() as isize)),
                    );
                }
                let initial = st
                    .target
                    .as_ref()
                    .map(|t| t.to_string())
                    .or_else(|| st.targets.first().cloned())
                    .unwrap_or_default();
                set_text(hwnd, ID_HOST, &initial);
                set_text(hwnd, ID_DIR, &yy_proto::display_path(&st.dir));
                set_text(hwnd, ID_NAME, &st.name);
                fill_encodings(hwnd);
                // 一覧の列（ダイアログ単位）
                let stops = [240i32];
                SendMessageW(
                    item(hwnd, ID_LIST),
                    LB_SETTABSTOPS,
                    Some(WPARAM(1)),
                    Some(LPARAM(stops.as_ptr() as isize)),
                );
                if st.target.is_some() {
                    // 前に使った接続先なら、開いたらすぐ一覧を出す
                    let _ = PostMessageW(
                        Some(hwnd),
                        WM_COMMAND,
                        WPARAM(ID_CONNECT as usize),
                        LPARAM(0),
                    );
                } else {
                    set_info(hwnd, "接続先を入力して「接続」を押してください");
                }
                1
            }
            WM_COMMAND => {
                let id = crate::loword(wparam.0) as u16;
                let code = crate::hiword(wparam.0);
                match id {
                    ID_CONNECT => connect(hwnd),
                    ID_LIST if code == LBN_DBLCLK => activate_row(hwnd),
                    ID_LIST if code == LBN_SELCHANGE => {
                        if let Some(Some(e)) = selected_row(hwnd)
                            && !e.info.as_ref().is_some_and(|i| i.is_dir())
                        {
                            set_text(hwnd, ID_NAME, &yy_proto::display_path(&e.name));
                        }
                    }
                    IDOK_ => {
                        // Enter は、入力中の欄に応じて接続・移動・決定する
                        let focus = GetFocus();
                        let host = item(hwnd, ID_HOST);
                        if focus == host || IsChild(host, focus).as_bool() {
                            connect(hwnd);
                        } else if focus == item(hwnd, ID_DIR) {
                            go_to_dir_field(hwnd);
                        } else if focus == item(hwnd, ID_LIST) {
                            activate_row(hwnd);
                        } else {
                            accept_name(hwnd);
                        }
                    }
                    IDCANCEL_ => {
                        let _ = EndDialog(hwnd, IDCANCEL_ as isize);
                    }
                    _ => return 0,
                }
                1
            }
            _ => 0,
        }
    }
}
