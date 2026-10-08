//! レイアウトカタログの UI（15 章 6.6）。
//!
//! カタログは 1 つのフォルダ（ルート。設定の `[sheet] layout_catalog`、空なら `%APPDATA%\yyeditor\layouts`）
//! の下に置いた定義ファイル（`.yyl`・コピーブック）。中身は [`yy_sheet::catalog`]。
//!
//! - レイアウトのダイアログ（固定長のレイアウト・マルチレイアウトの設定・固定長ファイルを開く）の
//!   「カタログから読み込む」「カタログに保存」で、コピーブックを貼り付ける代わりにカタログを使う。
//! - データ > レイアウトカタログから当てる: 選んだ定義を今のシートのダイアログに入れて開く。
//! - データ > シートのレイアウトをカタログに保存・レイアウトカタログのフォルダを開く。

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Gdi::{CreateFontW, DeleteObject, HFONT, HGDIOBJ};
use windows::Win32::UI::WindowsAndMessaging::*;
use yy_sheet::catalog::{self, Entry, Kind, LayoutDef};

use super::filter::{add_string, button, dlg_text, edit, label, message, run, send, state};
use super::*;
use crate::goto::{CLASS_EDIT, Template};

const CLASS_LISTBOX: u16 = 0x0083;
const IDOK_: u16 = 1;
const IDCANCEL_: u16 = 2;
const P_ROOT: u16 = 10;
const P_FILTER: u16 = 11;
const P_LIST: u16 = 12;
const P_PREVIEW: u16 = 13;
const P_FOLDER: u16 = 14;
const P_REFRESH: u16 = 15;
const S_NAME: u16 = 20;
const S_DESC: u16 = 21;
const S_LIST: u16 = 22;

thread_local! {
    /// カタログのルート（起動したときの設定から）
    static ROOT: RefCell<PathBuf> = RefCell::new(default_root());
    /// 最後に読み込んだ・保存した定義の名前（保存のダイアログの初期値）
    static LAST: RefCell<String> = const { RefCell::new(String::new()) };
    /// 次に開くレイアウトのダイアログに入れる定義（データ > レイアウトカタログから当てる）
    static PRESET: RefCell<Option<LayoutDef>> = const { RefCell::new(None) };
}

fn default_root() -> PathBuf {
    yy_config::config_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("layouts")
}

/// 設定からルートを決める（起動のとき）。
pub(super) fn set_root(config: &yy_config::Config) {
    let s = config.sheet.layout_catalog.trim();
    let root = if s.is_empty() {
        default_root()
    } else {
        PathBuf::from(s)
    };
    ROOT.with(|r| *r.borrow_mut() = root);
}

pub(super) fn root() -> PathBuf {
    ROOT.with(|r| r.borrow().clone())
}

/// [`PRESET`] を取り出す（レイアウトのダイアログの初期値）。
pub(super) fn take_preset() -> Option<LayoutDef> {
    PRESET.with(|p| p.borrow_mut().take())
}

fn kind_label(e: &Entry) -> &'static str {
    match e.kind {
        Kind::Definition => "",
        Kind::Copybook => "（コピーブック）",
    }
}

/// 定義の説明（一覧の右に出す）。
pub(super) fn describe(def: &LayoutDef) -> String {
    let mut lines = Vec::new();
    if !def.description.is_empty() {
        lines.push(def.description.clone());
        lines.push(String::new());
    }
    lines.push(format!(
        "種類: {}",
        if def.is_multi() {
            "マルチレイアウト"
        } else {
            "単一のレイアウト"
        }
    ));
    if let Some(n) = def.data_len {
        lines.push(format!("1 行のデータ長: {n} バイト"));
    }
    lines.push(format!(
        "文字コード: {}",
        def.charset
            .map_or("（ダイアログで選ぶ）".to_string(), |c| c.label())
    ));
    lines.push(format!(
        "レコードの区切り: {}",
        def.separator.map_or("（ダイアログで選ぶ）", |s| s.label())
    ));
    if let Some(le) = def.little_endian {
        lines.push(format!(
            "2 進数の並び: {}",
            if le {
                "リトルエンディアン"
            } else {
                "ビッグエンディアン"
            }
        ));
    }
    for (name, copybook) in &def.layouts {
        lines.push(String::new());
        let head = if name.is_empty() {
            "レイアウト".to_string()
        } else {
            format!("レイアウト {name}")
        };
        match yy_cobol::parse(copybook) {
            Ok(l) => lines.push(format!(
                "{head}: レコード {} バイト・項目 {} 個",
                l.record_len,
                l.fields.len()
            )),
            Err(e) => lines.push(format!("{head}: 読めません（{e}）")),
        }
        for l in copybook.lines().take(40) {
            lines.push(format!("  {l}"));
        }
        if copybook.lines().count() > 40 {
            lines.push("  …".into());
        }
    }
    lines.join("\r\n")
}

fn root_text(root: &Path) -> String {
    let mut t = format!("ルート: {}", root.display());
    if !root.is_dir() {
        t.push_str("（まだありません。保存すると作ります）");
    }
    t
}

fn open_folder(root: &Path) {
    let _ = std::fs::create_dir_all(root);
    unsafe {
        windows::Win32::UI::Shell::ShellExecuteW(
            None,
            &HSTRING::from("open"),
            &HSTRING::from(root.as_os_str()),
            None,
            None,
            SW_SHOWNORMAL,
        );
    }
}

/// 定義の文字コード・区切り・2 進数の並び（あるものだけ）をダイアログの欄に入れる。`sep_offset` は区切りの
/// 一覧の先頭の「自動」の数（開くときは 1）。
pub(super) fn set_codec_controls(
    hwnd: HWND,
    def: &LayoutDef,
    (charset_id, sep_id, le_id): (u16, u16, u16),
    sep_offset: usize,
) {
    use windows::Win32::UI::Controls::{BST_CHECKED, BST_UNCHECKED, CheckDlgButton};
    if let Some(c) = def.charset
        && let Some(i) = yy_cobol::Charset::all().iter().position(|x| *x == c)
    {
        send(hwnd, charset_id, CB_SETCURSEL, i, 0);
    }
    if let Some(s) = def.separator
        && let Some(i) = yy_sheet::fixed::RecordSep::ALL.iter().position(|x| *x == s)
    {
        send(hwnd, sep_id, CB_SETCURSEL, i + sep_offset, 0);
    }
    if let Some(le) = def.little_endian {
        let _ = unsafe {
            CheckDlgButton(
                hwnd,
                le_id as i32,
                if le { BST_CHECKED } else { BST_UNCHECKED },
            )
        };
    }
}

// ---- 読み込む ---------------------------------------------------------------------------------

struct PickState {
    root: PathBuf,
    entries: Vec<Entry>,
    /// 一覧に出している番号（`entries` の）
    shown: Vec<usize>,
    font: HFONT,
    result: Option<(String, LayoutDef)>,
}

fn reload(hwnd: HWND, st: &mut PickState) {
    st.entries = catalog::list(&st.root).unwrap_or_default();
    unsafe {
        let _ = SetDlgItemTextW(hwnd, P_ROOT as i32, &HSTRING::from(root_text(&st.root)));
    }
    refilter(hwnd, st);
}

fn refilter(hwnd: HWND, st: &mut PickState) {
    let f = dlg_text(hwnd, P_FILTER).to_lowercase();
    st.shown = (0..st.entries.len())
        .filter(|&i| f.is_empty() || st.entries[i].name.to_lowercase().contains(f.trim()))
        .collect();
    send(hwnd, P_LIST, LB_RESETCONTENT, 0, 0);
    for &i in &st.shown {
        let e = &st.entries[i];
        add_string(
            hwnd,
            P_LIST,
            LB_ADDSTRING,
            &format!("{}{}", e.name, kind_label(e)),
        );
    }
    let text = if st.entries.is_empty() {
        format!(
            "カタログに定義がありません。\r\n\r\n{} の下に、レイアウトの定義ファイル（.yyl）かコピーブック\
             （.cpy・.cbl・.cob・.copy）を置いてください。サブフォルダで分けても構いません。\r\n\
             レイアウトのダイアログの「カタログに保存」でも作れます。",
            st.root.display()
        )
    } else {
        String::new()
    };
    unsafe {
        let _ = SetDlgItemTextW(hwnd, P_PREVIEW as i32, &HSTRING::from(text));
    }
    if !st.shown.is_empty() {
        send(hwnd, P_LIST, LB_SETCURSEL, 0, 0);
        preview(hwnd, st);
    }
}

fn selected(hwnd: HWND, st: &PickState) -> Option<&Entry> {
    let i = usize::try_from(send(hwnd, P_LIST, LB_GETCURSEL, 0, 0)).ok()?;
    st.entries.get(*st.shown.get(i)?)
}

fn preview(hwnd: HWND, st: &PickState) {
    let Some(e) = selected(hwnd, st) else {
        return;
    };
    let text = match catalog::read_path(&e.path) {
        Ok(def) => format!(
            "{}\r\n{}\r\n\r\n{}",
            e.name,
            e.path.display(),
            describe(&def)
        ),
        Err(e) => format!("読めません: {e}"),
    };
    unsafe {
        let _ = SetDlgItemTextW(hwnd, P_PREVIEW as i32, &HSTRING::from(text));
    }
}

/// カタログから定義を選んで読む（名前と定義）。
pub(super) fn pick(owner: HWND) -> Option<(String, LayoutDef)> {
    let mut t = Template::dialog("レイアウトカタログから読み込む", 460, 280);
    label(&mut t, 7, 7, 446, P_ROOT, "");
    label(&mut t, 7, 23, 40, 0, "絞り込み:");
    edit(&mut t, 47, 21, 140, P_FILTER);
    t.item(
        (WS_BORDER | WS_TABSTOP | WS_VSCROLL | WS_HSCROLL).0 | (LBS_NOTIFY | LBS_HASSTRINGS) as u32,
        7,
        38,
        180,
        214,
        P_LIST,
        CLASS_LISTBOX,
        "",
    );
    t.item(
        (WS_BORDER | WS_VSCROLL | WS_HSCROLL).0
            | (ES_MULTILINE | ES_AUTOVSCROLL | ES_AUTOHSCROLL | ES_READONLY) as u32,
        193,
        21,
        260,
        231,
        P_PREVIEW,
        CLASS_EDIT,
        "",
    );
    button(&mut t, 7, 259, 90, P_FOLDER, "フォルダを開く(&O)", false);
    button(
        &mut t,
        101,
        259,
        86,
        P_REFRESH,
        "最新の情報に更新(&R)",
        false,
    );
    button(&mut t, 346, 259, 50, IDOK_, "読み込む", true);
    button(&mut t, 403, 259, 50, IDCANCEL_, "キャンセル", false);
    let mut st = PickState {
        root: root(),
        entries: Vec::new(),
        shown: Vec::new(),
        font: HFONT::default(),
        result: None,
    };
    run(&t, owner, &mut st, Some(pick_proc));
    let r = st.result;
    if let Some((name, _)) = &r {
        LAST.with(|l| *l.borrow_mut() = name.clone());
    }
    r
}

fn mono_font(hwnd: HWND) -> HFONT {
    let dpi = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd) }.max(96) as i32;
    unsafe {
        CreateFontW(
            -12 * dpi / 96,
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            windows::Win32::Graphics::Gdi::DEFAULT_CHARSET,
            windows::Win32::Graphics::Gdi::OUT_DEFAULT_PRECIS,
            windows::Win32::Graphics::Gdi::CLIP_DEFAULT_PRECIS,
            windows::Win32::Graphics::Gdi::CLEARTYPE_QUALITY,
            0,
            w!("MS Gothic"),
        )
    }
}

extern "system" fn pick_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> isize {
    unsafe {
        match msg {
            WM_INITDIALOG => {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, lparam.0);
                let st = state::<PickState>(hwnd);
                st.font = mono_font(hwnd);
                send(hwnd, P_PREVIEW, WM_SETFONT, st.font.0 as usize, 1);
                reload(hwnd, st);
                1
            }
            WM_DESTROY if GetWindowLongPtrW(hwnd, GWLP_USERDATA) != 0 => {
                let st = state::<PickState>(hwnd);
                let _ = DeleteObject(HGDIOBJ(st.font.0));
                0
            }
            WM_COMMAND if GetWindowLongPtrW(hwnd, GWLP_USERDATA) == 0 => 0,
            WM_COMMAND => {
                let id = crate::loword(wparam.0) as u16;
                let code = crate::hiword(wparam.0);
                let st = state::<PickState>(hwnd);
                match id {
                    P_FILTER if code == EN_CHANGE => {
                        refilter(hwnd, st);
                        1
                    }
                    P_LIST if code == LBN_SELCHANGE => {
                        preview(hwnd, st);
                        1
                    }
                    P_LIST if code == LBN_DBLCLK => {
                        let _ =
                            PostMessageW(Some(hwnd), WM_COMMAND, WPARAM(IDOK_ as usize), LPARAM(0));
                        1
                    }
                    P_FOLDER => {
                        open_folder(&st.root);
                        1
                    }
                    P_REFRESH => {
                        reload(hwnd, st);
                        1
                    }
                    IDOK_ => {
                        let Some(e) = selected(hwnd, st).cloned() else {
                            message(hwnd, "定義を選んでください。");
                            return 1;
                        };
                        match catalog::read_path(&e.path) {
                            Ok(def) => {
                                st.result = Some((e.name, def));
                                let _ = EndDialog(hwnd, IDOK_ as isize);
                            }
                            Err(err) => message(hwnd, &format!("読み込めません。\n{err}")),
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

// ---- 保存する ---------------------------------------------------------------------------------

struct SaveState {
    root: PathBuf,
    entries: Vec<Entry>,
    def: LayoutDef,
    name: String,
    result: Option<String>,
}

/// 定義をカタログに保存する（名前と説明を尋ねる）。保存した名前を返す。
pub(super) fn save(owner: HWND, def: LayoutDef) -> Option<String> {
    let mut t = Template::dialog("レイアウトカタログに保存", 400, 230);
    label(&mut t, 7, 7, 386, P_ROOT, "");
    label(
        &mut t,
        7,
        22,
        386,
        0,
        "名前（ルートからの相対パス。例: 受注/ORDER。拡張子がなければ .yyl、.cpy ならコピーブックだけ）:",
    );
    edit(&mut t, 7, 34, 386, S_NAME);
    label(&mut t, 7, 52, 40, 0, "説明:");
    edit(&mut t, 47, 50, 346, S_DESC);
    label(
        &mut t,
        7,
        68,
        386,
        0,
        "カタログにある定義（選ぶと名前に入ります。同じ名前なら上書きします）:",
    );
    t.item(
        (WS_BORDER | WS_TABSTOP | WS_VSCROLL | WS_HSCROLL).0 | (LBS_NOTIFY | LBS_HASSTRINGS) as u32,
        7,
        80,
        386,
        124,
        S_LIST,
        CLASS_LISTBOX,
        "",
    );
    button(&mut t, 286, 209, 50, IDOK_, "保存", true);
    button(&mut t, 343, 209, 50, IDCANCEL_, "キャンセル", false);
    let root = root();
    let mut st = SaveState {
        entries: catalog::list(&root).unwrap_or_default(),
        root,
        def,
        name: LAST.with(|l| l.borrow().clone()),
        result: None,
    };
    run(&t, owner, &mut st, Some(save_proc));
    let r = st.result;
    if let Some(name) = &r {
        LAST.with(|l| *l.borrow_mut() = name.clone());
    }
    r
}

extern "system" fn save_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> isize {
    unsafe {
        match msg {
            WM_INITDIALOG => {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, lparam.0);
                let st = state::<SaveState>(hwnd);
                let _ = SetDlgItemTextW(hwnd, P_ROOT as i32, &HSTRING::from(root_text(&st.root)));
                let _ = SetDlgItemTextW(hwnd, S_NAME as i32, &HSTRING::from(st.name.as_str()));
                let _ = SetDlgItemTextW(
                    hwnd,
                    S_DESC as i32,
                    &HSTRING::from(st.def.description.as_str()),
                );
                for e in &st.entries {
                    add_string(
                        hwnd,
                        S_LIST,
                        LB_ADDSTRING,
                        &format!("{}{}", e.name, kind_label(e)),
                    );
                }
                1
            }
            WM_COMMAND if GetWindowLongPtrW(hwnd, GWLP_USERDATA) == 0 => 0,
            WM_COMMAND => {
                let id = crate::loword(wparam.0) as u16;
                let code = crate::hiword(wparam.0);
                let st = state::<SaveState>(hwnd);
                match id {
                    S_LIST if code == LBN_SELCHANGE => {
                        if let Ok(i) = usize::try_from(send(hwnd, S_LIST, LB_GETCURSEL, 0, 0))
                            && let Some(e) = st.entries.get(i)
                        {
                            let _ = SetDlgItemTextW(
                                hwnd,
                                S_NAME as i32,
                                &HSTRING::from(e.name.as_str()),
                            );
                            // 説明は今のものを残す（空なら、選んだ定義のもの）
                            if dlg_text(hwnd, S_DESC).trim().is_empty()
                                && let Ok(d) = catalog::read_path(&e.path)
                            {
                                let _ = SetDlgItemTextW(
                                    hwnd,
                                    S_DESC as i32,
                                    &HSTRING::from(d.description.as_str()),
                                );
                            }
                        }
                        1
                    }
                    IDOK_ => {
                        let name = dlg_text(hwnd, S_NAME);
                        let path = match catalog::resolve(&st.root, &name) {
                            Ok(p) => p,
                            Err(e) => {
                                message(hwnd, &e);
                                return 1;
                            }
                        };
                        if path.exists() {
                            let r = MessageBoxW(
                                Some(hwnd),
                                &HSTRING::from(format!(
                                    "{} はもうあります。上書きしますか？",
                                    path.display()
                                )),
                                w!("yysheet"),
                                MB_YESNO | MB_ICONQUESTION | MB_DEFBUTTON2,
                            );
                            if r != IDYES {
                                return 1;
                            }
                        }
                        let mut def = st.def.clone();
                        def.description = dlg_text(hwnd, S_DESC).trim().to_string();
                        match catalog::write(&st.root, &name, &def) {
                            Ok(p) => {
                                st.result =
                                    Some(catalog::name_of(&st.root, &p).unwrap_or(name.clone()));
                                let _ = EndDialog(hwnd, IDOK_ as isize);
                            }
                            Err(e) => message(hwnd, &format!("保存できません。\n{e}")),
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

// ---- メニュー ---------------------------------------------------------------------------------

/// データ > レイアウトカタログから当てる: 選んだ定義を、今のシートのレイアウトのダイアログに入れて開く。
pub(super) fn apply_from_catalog() {
    let Some(frame) = with(|a| {
        a.end_edit(true);
        a.frame
    }) else {
        return;
    };
    let Some((name, def)) = pick(frame) else {
        return;
    };
    let multi = def.is_multi();
    PRESET.with(|p| *p.borrow_mut() = Some(def));
    set_status(&format!(
        "カタログの {name} を読み込みました。確かめて OK で当てます"
    ));
    if multi {
        super::multiui::multi_layout_dialog();
    } else {
        super::fixedui::layout_dialog();
    }
    // ダイアログが使わなかったときのために消す
    PRESET.with(|p| p.borrow_mut().take());
}

/// データ > シートのレイアウトをカタログに保存。
pub(super) fn save_sheet_layout() {
    let Some((frame, spec)) = with(|a| {
        a.end_edit(true);
        (a.frame, a.sheet().fixed.as_deref().cloned())
    }) else {
        return;
    };
    let Some(spec) = spec else {
        info_box(
            frame,
            "このシートには固定長のレイアウトがありません。\n\
             データ > 固定長のレイアウト（またはマルチレイアウトの設定）で設定してください。",
        );
        return;
    };
    if let Some(name) = save(frame, LayoutDef::from_spec(&spec, "")) {
        set_status(&format!("レイアウトをカタログの {name} に保存しました"));
    }
}

/// データ > レイアウトカタログのフォルダを開く。
pub(super) fn open_root_folder() {
    open_folder(&root());
}
