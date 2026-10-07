//! 3270 のファイル転送（IND$FILE）のダイアログ（14 章 11 節）。
//!
//! 向き（受け取る・送る）、ホストの種類、ホストのファイル、手元のファイル、データの扱い、
//! 送るときの新しいファイルの形式を尋ねる。

use std::path::PathBuf;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Controls::{
    BST_CHECKED, CheckDlgButton, CheckRadioButton, IsDlgButtonChecked,
};
use windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, w};
use yy_3270::ind_file::{Direction, HostKind, Mode, Recfm, Request};
use yy_encoding::Ccsid;

use crate::goto::{CLASS_BUTTON, CLASS_EDIT, CLASS_STATIC, Template};

const CLASS_COMBOBOX: u16 = 0x0085;

const ID_GET: u16 = 100;
const ID_PUT: u16 = 101;
const ID_TSO: u16 = 110;
const ID_CMS: u16 = 111;
const ID_CICS: u16 = 112;
const ID_HOST_FILE: u16 = 120;
const ID_LOCAL_FILE: u16 = 121;
const ID_BROWSE: u16 = 122;
const ID_TEXT: u16 = 130;
const ID_ASCII: u16 = 131;
const ID_BINARY: u16 = 132;
const ID_RECFM: u16 = 140;
const ID_LRECL: u16 = 141;
const ID_SPACE: u16 = 142;
const ID_APPEND: u16 = 143;
const ID_OPEN_AFTER: u16 = 150;
const ID_PUT_LABELS: [u16; 3] = [160, 161, 162];
const IDOK_: u16 = 1;
const IDCANCEL_: u16 = 2;

/// ダイアログで選んだ転送。
#[derive(Clone, Debug)]
pub(crate) struct Choice {
    pub request: Request,
    pub local: PathBuf,
    /// 受け取ったら yyeditor で開く
    pub open_after: bool,
}

impl Choice {
    pub(crate) fn initial(ccsid: Ccsid) -> Choice {
        Choice {
            request: Request {
                host: HostKind::Tso,
                direction: Direction::Receive,
                host_file: String::new(),
                mode: Mode::Text(ccsid),
                recfm: Recfm::Default,
                lrecl: 0,
                space: 0,
                append: false,
            },
            local: PathBuf::new(),
            open_after: true,
        }
    }
}

struct State {
    choice: Choice,
    ccsid: Ccsid,
}

const RECFMS: [(Recfm, &str); 4] = [
    (Recfm::Default, "ホストに任せる"),
    (Recfm::Fixed, "F（固定長）"),
    (Recfm::Variable, "V（可変長）"),
    (Recfm::Undefined, "U（不定形式）"),
];

fn build_template(ccsid: Ccsid) -> Template {
    let mut t = Template::dialog("3270 のファイル転送（IND$FILE）", 300, 222);
    let radio = |t: &mut Template, first: bool, x, y, cx, id, text: &str| {
        let style = if first {
            (WS_GROUP | WS_TABSTOP).0 | BS_AUTORADIOBUTTON as u32
        } else {
            BS_AUTORADIOBUTTON as u32
        };
        t.item(style, x, y, cx, 11, id, CLASS_BUTTON, text);
    };
    t.item(0, 7, 9, 50, 10, 0xFFFF, CLASS_STATIC, "向き:");
    radio(&mut t, true, 60, 8, 110, ID_GET, "ホストから受け取る (GET)");
    radio(&mut t, false, 175, 8, 110, ID_PUT, "ホストへ送る (PUT)");
    t.item(0, 7, 24, 50, 10, 0xFFFF, CLASS_STATIC, "ホスト:");
    radio(&mut t, true, 60, 23, 50, ID_TSO, "TSO");
    radio(&mut t, false, 115, 23, 50, ID_CMS, "CMS");
    radio(&mut t, false, 170, 23, 50, ID_CICS, "CICS");
    t.item(
        0,
        7,
        40,
        286,
        10,
        0xFFFF,
        CLASS_STATIC,
        "ホストのファイル（TSO: 'USER.DATA(MEM)'、CMS: PROFILE EXEC A）:",
    );
    t.item(
        (WS_GROUP | WS_TABSTOP | WS_BORDER).0 | ES_AUTOHSCROLL as u32,
        7,
        51,
        286,
        13,
        ID_HOST_FILE,
        CLASS_EDIT,
        "",
    );
    t.item(0, 7, 69, 286, 10, 0xFFFF, CLASS_STATIC, "手元のファイル:");
    t.item(
        (WS_TABSTOP | WS_BORDER).0 | ES_AUTOHSCROLL as u32,
        7,
        80,
        232,
        13,
        ID_LOCAL_FILE,
        CLASS_EDIT,
        "",
    );
    t.item(
        WS_TABSTOP.0 | BS_PUSHBUTTON as u32,
        243,
        79,
        50,
        14,
        ID_BROWSE,
        CLASS_BUTTON,
        "参照(&B)...",
    );
    t.item(0, 7, 99, 50, 10, 0xFFFF, CLASS_STATIC, "データ:");
    radio(
        &mut t,
        true,
        60,
        98,
        233,
        ID_TEXT,
        &format!(
            "テキスト（バイナリで転送し、端末で {} と変換する。日本語）",
            ccsid.name()
        ),
    );
    radio(
        &mut t,
        false,
        60,
        111,
        233,
        ID_ASCII,
        "テキスト（ホストの ASCII 変換。ASCII CRLF）",
    );
    radio(
        &mut t,
        false,
        60,
        124,
        233,
        ID_BINARY,
        "バイナリ（変換しない）",
    );
    t.item(
        0,
        7,
        142,
        286,
        10,
        ID_PUT_LABELS[0],
        CLASS_STATIC,
        "送るとき、新しく作るファイルの形式（空欄はホストに任せる）:",
    );
    t.item(0, 14, 157, 34, 10, ID_PUT_LABELS[1], CLASS_STATIC, "RECFM:");
    t.item(
        (WS_GROUP | WS_TABSTOP | WS_VSCROLL).0 | CBS_DROPDOWNLIST as u32,
        48,
        155,
        80,
        80,
        ID_RECFM,
        CLASS_COMBOBOX,
        "",
    );
    t.item(
        0,
        134,
        157,
        30,
        10,
        ID_PUT_LABELS[2],
        CLASS_STATIC,
        "LRECL:",
    );
    t.item(
        (WS_TABSTOP | WS_BORDER).0 | (ES_NUMBER | ES_AUTOHSCROLL) as u32,
        164,
        155,
        36,
        13,
        ID_LRECL,
        CLASS_EDIT,
        "",
    );
    t.item(0, 206, 157, 52, 10, 0xFFFF, CLASS_STATIC, "トラック数:");
    t.item(
        (WS_TABSTOP | WS_BORDER).0 | (ES_NUMBER | ES_AUTOHSCROLL) as u32,
        257,
        155,
        36,
        13,
        ID_SPACE,
        CLASS_EDIT,
        "",
    );
    t.item(
        WS_TABSTOP.0 | BS_AUTOCHECKBOX as u32,
        14,
        173,
        200,
        11,
        ID_APPEND,
        CLASS_BUTTON,
        "既にあるファイルに追加する (APPEND)",
    );
    t.item(
        WS_TABSTOP.0 | BS_AUTOCHECKBOX as u32,
        7,
        188,
        286,
        11,
        ID_OPEN_AFTER,
        CLASS_BUTTON,
        "受け取ったら yyeditor で開く",
    );
    t.item(
        (WS_GROUP | WS_TABSTOP).0 | BS_DEFPUSHBUTTON as u32,
        189,
        203,
        50,
        14,
        IDOK_,
        CLASS_BUTTON,
        "転送",
    );
    t.item(
        WS_TABSTOP.0 | BS_PUSHBUTTON as u32,
        243,
        203,
        50,
        14,
        IDCANCEL_,
        CLASS_BUTTON,
        "キャンセル",
    );
    t
}

/// 転送の指定を尋ねる。`last` は前回の指定（初期値）。キャンセルされたら `None`。
pub(crate) fn show(owner: HWND, last: Choice, ccsid: Ccsid) -> Option<Choice> {
    let aligned = build_template(ccsid).aligned();
    let mut state = State {
        choice: last,
        ccsid,
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
    (r == IDOK_ as isize).then_some(state.choice)
}

fn checked(hwnd: HWND, id: u16) -> bool {
    unsafe { IsDlgButtonChecked(hwnd, id as i32) == BST_CHECKED.0 }
}

fn text(hwnd: HWND, id: u16) -> String {
    let mut buf = vec![0u16; 1024];
    let n = unsafe { GetDlgItemTextW(hwnd, id as i32, &mut buf) } as usize;
    String::from_utf16_lossy(&buf[..n]).trim().to_owned()
}

fn set_text(hwnd: HWND, id: u16, s: &str) {
    unsafe {
        let _ = SetDlgItemTextW(hwnd, id as i32, &HSTRING::from(s));
    }
}

fn number(n: u32) -> String {
    if n == 0 { String::new() } else { n.to_string() }
}

/// 送るときだけの欄を、向きに合わせて使える・使えないにする。
fn update_enabled(hwnd: HWND) {
    let put = checked(hwnd, ID_PUT);
    let tso = checked(hwnd, ID_TSO);
    let cics = checked(hwnd, ID_CICS);
    let set = |id: u16, on: bool| unsafe {
        if let Ok(w) = GetDlgItem(Some(hwnd), id as i32) {
            let _ = EnableWindow(w, on);
        }
    };
    for id in ID_PUT_LABELS {
        set(id, put && !cics);
    }
    set(ID_RECFM, put && !cics);
    // 受け取るときの LRECL は、ホストが区切りを入れない固定長のレコードを分ける長さ
    set(ID_LRECL, !cics);
    set(ID_PUT_LABELS[2], !cics);
    set(ID_SPACE, put && tso);
    set(ID_APPEND, put);
    set(ID_OPEN_AFTER, !put);
}

fn warn(hwnd: HWND, msg: &str) {
    unsafe {
        let _ = MessageBoxW(
            Some(hwnd),
            &HSTRING::from(msg),
            w!("yyterm"),
            MB_OK | MB_ICONWARNING,
        );
    }
}

/// 手元のファイルを選ぶ（受け取るときは保存先、送るときは送るファイル）。
fn browse(hwnd: HWND) {
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree};
    use windows::Win32::UI::Shell::{
        FileOpenDialog, FileSaveDialog, IFileDialog, IFileOpenDialog, IFileSaveDialog,
        SIGDN_FILESYSPATH,
    };
    use windows::core::Interface;
    let put = checked(hwnd, ID_PUT);
    let current = text(hwnd, ID_LOCAL_FILE);
    unsafe {
        let dialog: Option<IFileDialog> = if put {
            CoCreateInstance::<_, IFileOpenDialog>(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)
                .ok()
                .and_then(|d| d.cast().ok())
        } else {
            CoCreateInstance::<_, IFileSaveDialog>(&FileSaveDialog, None, CLSCTX_INPROC_SERVER)
                .ok()
                .and_then(|d| d.cast().ok())
        };
        let Some(d) = dialog else { return };
        let _ = d.SetTitle(if put {
            w!("ホストへ送るファイル")
        } else {
            w!("受け取ったデータを保存するファイル")
        });
        if !current.is_empty()
            && let Some(name) = std::path::Path::new(&current).file_name()
        {
            let _ = d.SetFileName(&HSTRING::from(name.to_string_lossy().as_ref()));
        }
        if d.Show(Some(hwnd)).is_err() {
            return;
        }
        if let Ok(item) = d.GetResult()
            && let Ok(name) = item.GetDisplayName(SIGDN_FILESYSPATH)
        {
            if let Ok(s) = name.to_string() {
                set_text(hwnd, ID_LOCAL_FILE, &s);
            }
            CoTaskMemFree(Some(name.0 as *const _));
        }
    }
}

/// 入力を読み取る。足りなければ理由を返す。
fn read_choice(hwnd: HWND, ccsid: Ccsid) -> Result<Choice, String> {
    let direction = if checked(hwnd, ID_PUT) {
        Direction::Send
    } else {
        Direction::Receive
    };
    let host = if checked(hwnd, ID_CMS) {
        HostKind::Cms
    } else if checked(hwnd, ID_CICS) {
        HostKind::Cics
    } else {
        HostKind::Tso
    };
    let host_file = text(hwnd, ID_HOST_FILE);
    if host_file.is_empty() {
        return Err("ホストのファイルを入力してください。".into());
    }
    let local = text(hwnd, ID_LOCAL_FILE);
    if local.is_empty() {
        return Err("手元のファイルを入力してください。".into());
    }
    let local = PathBuf::from(local);
    if direction == Direction::Send && !local.is_file() {
        return Err(format!("{} がありません。", local.display()));
    }
    let mode = if checked(hwnd, ID_ASCII) {
        Mode::HostAscii
    } else if checked(hwnd, ID_BINARY) {
        Mode::Binary
    } else {
        Mode::Text(ccsid)
    };
    let sel = unsafe {
        SendMessageW(
            GetDlgItem(Some(hwnd), ID_RECFM as i32).unwrap_or_default(),
            CB_GETCURSEL,
            None,
            None,
        )
        .0
    };
    let recfm = RECFMS
        .get(usize::try_from(sel).unwrap_or(0))
        .map_or(Recfm::Default, |r| r.0);
    let num = |id: u16, what: &str| -> Result<u32, String> {
        let s = text(hwnd, id);
        if s.is_empty() {
            return Ok(0);
        }
        s.parse::<u32>()
            .ok()
            .filter(|&n| n <= 32760)
            .ok_or_else(|| format!("{what}は 0 〜 32760 で指定してください。"))
    };
    Ok(Choice {
        request: Request {
            host,
            direction,
            host_file,
            mode,
            recfm,
            lrecl: num(ID_LRECL, "LRECL ")?,
            space: num(ID_SPACE, "トラック数")?,
            append: checked(hwnd, ID_APPEND),
        },
        local,
        open_after: checked(hwnd, ID_OPEN_AFTER),
    })
}

extern "system" fn dialog_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> isize {
    unsafe {
        match msg {
            WM_INITDIALOG => {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, lparam.0);
                let st = &*(lparam.0 as *const State);
                let c = &st.choice;
                let r = &c.request;
                let dir = match r.direction {
                    Direction::Receive => ID_GET,
                    Direction::Send => ID_PUT,
                };
                let _ = CheckRadioButton(hwnd, ID_GET as i32, ID_PUT as i32, dir as i32);
                let host = match r.host {
                    HostKind::Tso => ID_TSO,
                    HostKind::Cms => ID_CMS,
                    HostKind::Cics => ID_CICS,
                };
                let _ = CheckRadioButton(hwnd, ID_TSO as i32, ID_CICS as i32, host as i32);
                let mode = match r.mode {
                    Mode::Text(_) => ID_TEXT,
                    Mode::HostAscii => ID_ASCII,
                    Mode::Binary => ID_BINARY,
                };
                let _ = CheckRadioButton(hwnd, ID_TEXT as i32, ID_BINARY as i32, mode as i32);
                set_text(hwnd, ID_HOST_FILE, &r.host_file);
                set_text(hwnd, ID_LOCAL_FILE, &c.local.to_string_lossy());
                set_text(hwnd, ID_LRECL, &number(r.lrecl));
                set_text(hwnd, ID_SPACE, &number(r.space));
                let combo = GetDlgItem(Some(hwnd), ID_RECFM as i32).unwrap_or_default();
                let mut selected = 0;
                for (i, (f, label)) in RECFMS.iter().enumerate() {
                    SendMessageW(
                        combo,
                        CB_ADDSTRING,
                        None,
                        Some(LPARAM(HSTRING::from(*label).as_ptr() as isize)),
                    );
                    if *f == r.recfm {
                        selected = i;
                    }
                }
                SendMessageW(combo, CB_SETCURSEL, Some(WPARAM(selected)), None);
                let check = |id: u16, on: bool| {
                    let _ = CheckDlgButton(
                        hwnd,
                        id as i32,
                        if on {
                            BST_CHECKED
                        } else {
                            windows::Win32::UI::Controls::BST_UNCHECKED
                        },
                    );
                };
                check(ID_APPEND, r.append);
                check(ID_OPEN_AFTER, c.open_after);
                update_enabled(hwnd);
                1
            }
            WM_COMMAND => {
                let id = crate::loword(wparam.0) as u16;
                match id {
                    ID_GET | ID_PUT | ID_TSO | ID_CMS | ID_CICS => {
                        update_enabled(hwnd);
                        0
                    }
                    ID_BROWSE => {
                        browse(hwnd);
                        1
                    }
                    IDOK_ => {
                        let st = &mut *(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State);
                        match read_choice(hwnd, st.ccsid) {
                            Ok(c) => {
                                st.choice = c;
                                let _ = EndDialog(hwnd, IDOK_ as isize);
                            }
                            Err(e) => warn(hwnd, &e),
                        }
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
