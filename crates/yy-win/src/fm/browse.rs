//! 「フォルダ」のタブ（2026-10-27 の提案 4〜6・8 章）: エクスプローラーの代わりにフォルダを開いて見る。
//!
//! 上にアドレスバー・戻る/進む/上へ・絞り込み、左にツリー（よく使うフォルダ・PC・ピン留め）、右に一覧
//! （仮想リスト。システムのアイコン・列の見出しで並べ替え）。フォルダの読み込みは作業スレッドで行い、
//! 見つけた順に一覧へ足す。
//!
//! - フラットビュー: 開いているフォルダの下のすべてのファイルを 1 つの一覧に出す（「フォルダ」の列）。
//! - 圧縮ファイル（ZIP・tar・7z など）は展開せずにフォルダのように中に入る（`yy_files::archive`）。
//! - 絞り込みは検索のタブと同じ書き方（`ext:`・`size:` など）に、メタデータの条件（`exif.model:`）を足す。
//! - コピー・移動・削除・名前の変更・右クリックのメニュー・クリップボード・ドラッグは Windows のシェル
//!   （[`super::shell`]）。

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{HFONT, InvalidateRect, ScreenToClient};
use windows::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL, FILE_FLAGS_AND_ATTRIBUTES,
    GetDiskFreeSpaceExW, GetLogicalDrives,
};
use windows::Win32::System::Threading::WaitForSingleObject;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::{
    DragAcceptFiles, DragFinish, HDROP, SHFILEINFOW, SHGFI_LARGEICON, SHGFI_SMALLICON,
    SHGFI_SYSICONINDEX, SHGFI_TYPENAME, SHGFI_USEFILEATTRIBUTES, SHGetFileInfoW,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PCWSTR, PWSTR, w};
use yy_files::archive::{Archive, Kind};
use yy_files::jobs::{BrowseState, Dirs};
use yy_files::props::Props;
use yy_files::search::Query;

use super::shell::{self, Op};
use crate::util::error_box;
use crate::{hiword, loword};

const CLASS: PCWSTR = w!("YYFmBrowse");

// 作業スレッドからの知らせ（fm の WM_APP + 90 と重ならない）
const WM_APP_ROWS: u32 = WM_APP + 100;
const WM_APP_TREE: u32 = WM_APP + 101;
const WM_APP_ARCHIVE: u32 = WM_APP + 102;
const WM_APP_CHANGED: u32 = WM_APP + 103;
const WM_APP_PROPS: u32 = WM_APP + 104;
const WM_APP_EXTRACTED: u32 = WM_APP + 105;
/// 一括リネームの後などに読み直す
pub(super) const WM_APP_RELOAD: u32 = WM_APP + 106;

const TIMER_FILTER: usize = 1;
const TIMER_RELOAD: usize = 2;

// 部品の ID
const ID_BACK: u16 = 6001;
const ID_FORWARD: u16 = 6002;
const ID_UP: u16 = 6003;
const ID_REFRESH: u16 = 6004;
const ID_ADDRESS: u16 = 6005;
const ID_GO: u16 = 6006;
const ID_FILTER: u16 = 6007;
const ID_FLAT: u16 = 6008;
const ID_RENAME_MANY: u16 = 6009;
const ID_VIEW: u16 = 6010;
const ID_NEW_FOLDER: u16 = 6011;
const ID_TREE: u16 = 6012;
const ID_LIST: u16 = 6013;

// 右クリックの項目（シェルのメニューは shell::SHELL_FIRST から）
const CM_OPEN: u32 = 1;
const CM_OPEN_WITH: u32 = 2;
const CM_EDITOR: u32 = 3;
const CM_TERMINAL: u32 = 4;
const CM_EXPLORER: u32 = 5;
const CM_LOCATION: u32 = 6;
const CM_COPY_PATH: u32 = 7;
const CM_CUT: u32 = 8;
const CM_COPY: u32 = 9;
const CM_PASTE: u32 = 10;
const CM_RENAME: u32 = 11;
const CM_RENAME_MANY: u32 = 12;
const CM_DELETE: u32 = 13;
const CM_NEW_FOLDER: u32 = 14;
const CM_COPY_TO: u32 = 15;
const CM_MOVE_TO: u32 = 16;
const CM_EXTRACT: u32 = 17;
const CM_PIN: u32 = 18;
const CM_UNPIN: u32 = 19;
const CM_PROPS: u32 = 20;
const CM_SELECT_ALL: u32 = 21;
const CM_INVERT: u32 = 22;
const CM_REFRESH: u32 = 23;
const CM_FLAT: u32 = 24;
// 表示のメニュー
const VM_DETAILS: u32 = 101;
const VM_LIST: u32 = 102;
const VM_ICONS: u32 = 103;
const VM_HIDDEN: u32 = 104;
const VM_EXT: u32 = 105;
const VM_FLAT_FOLDERS: u32 = 106;

/// 境界の幅（96 DPI）
const SPLITTER: i32 = 5;
/// 圧縮ファイルの中をドラッグで出すときの大きさの上限（一時フォルダに展開するため）
const DRAG_EXTRACT_MAX: u64 = 512 << 20;

/// 開いている場所。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Loc {
    /// PC（ドライブの一覧）
    Computer,
    Folder(PathBuf),
    /// 圧縮ファイルの中（`inner` は `/` 区切り。一番上は空）
    Archive {
        file: PathBuf,
        inner: String,
    },
}

impl Loc {
    fn address(&self) -> String {
        match self {
            Loc::Computer => "PC".into(),
            Loc::Folder(p) => p.to_string_lossy().into_owned(),
            Loc::Archive { file, inner } => {
                let mut s = file.to_string_lossy().into_owned();
                if !inner.is_empty() {
                    s.push('\\');
                    s.push_str(&inner.replace('/', "\\"));
                }
                s
            }
        }
    }

    fn parent(&self) -> Option<Loc> {
        match self {
            Loc::Computer => None,
            Loc::Folder(p) => Some(match p.parent() {
                Some(q) if !q.as_os_str().is_empty() => Loc::Folder(q.to_path_buf()),
                _ => Loc::Computer,
            }),
            Loc::Archive { file, inner } => Some(if inner.is_empty() {
                file.parent()
                    .map_or(Loc::Computer, |p| Loc::Folder(p.to_path_buf()))
            } else {
                Loc::Archive {
                    file: file.clone(),
                    inner: inner
                        .rsplit_once('/')
                        .map_or(String::new(), |(a, _)| a.to_owned()),
                }
            }),
        }
    }

    /// 打たれたアドレスを読む（`PC`、フォルダ、`資料.zip\中のフォルダ`、環境変数）。
    fn parse(s: &str) -> Option<Loc> {
        let s = s.trim().trim_matches('"');
        if s.is_empty() {
            return None;
        }
        if s.eq_ignore_ascii_case("pc") || s == "コンピューター" {
            return Some(Loc::Computer);
        }
        let s = expand_env(s);
        let p = PathBuf::from(&s);
        if p.is_dir() {
            return Some(Loc::Folder(p));
        }
        if p.is_file() {
            if Kind::of(&s).is_some() {
                return Some(Loc::Archive {
                    file: p,
                    inner: String::new(),
                });
            }
            return p.parent().map(|q| Loc::Folder(q.to_path_buf()));
        }
        // 圧縮ファイルの中のパス
        let mut cur = p.as_path();
        let mut rest: Vec<String> = Vec::new();
        while let Some(parent) = cur.parent() {
            if let Some(n) = cur.file_name() {
                rest.push(n.to_string_lossy().into_owned());
            }
            cur = parent;
            if cur.is_file() && Kind::of(&cur.to_string_lossy()).is_some() {
                rest.reverse();
                return Some(Loc::Archive {
                    file: cur.to_path_buf(),
                    inner: rest.join("/"),
                });
            }
            if cur.is_dir() {
                break;
            }
        }
        None
    }
}

/// `%USERPROFILE%` などを展開する。
fn expand_env(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('%') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        match after.find('%') {
            Some(j) => {
                let name = &after[..j];
                match std::env::var(name) {
                    Ok(v) if !name.is_empty() => out.push_str(&v),
                    _ => {
                        out.push('%');
                        out.push_str(name);
                        out.push('%');
                    }
                }
                rest = &after[j + 1..];
            }
            None => {
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// 一覧の 1 行。
#[derive(Clone, Debug, Default)]
struct Row {
    name: String,
    /// フラットビュー: 開いている場所からのフォルダ（`\` 区切り）
    folder: String,
    /// 本当のパス（圧縮ファイルの中なら圧縮ファイル）
    path: PathBuf,
    /// 圧縮ファイルの中の項目の番号
    entry: Option<usize>,
    dir: bool,
    size: u64,
    /// 更新日時・作成日時（UNIX 時刻からのナノ秒）
    mtime: i64,
    ctime: i64,
    attrs: u32,
    /// 隠し（自分か、フラットビューで途中のフォルダが隠し・システム）
    hidden: bool,
    /// ドライブ（合計・空き）
    drive: Option<(u64, u64)>,
}

const ATTR_READONLY: u32 = 0x1;
const ATTR_HIDDEN: u32 = 0x2;
const ATTR_SYSTEM: u32 = 0x4;
const ATTR_DIRECTORY: u32 = 0x10;
const ATTR_ARCHIVE: u32 = 0x20;
const ATTR_REPARSE: u32 = 0x400;
const ATTR_COMPRESSED: u32 = 0x800;
const ATTR_ENCRYPTED: u32 = 0x4000;

/// FILETIME（1601 年からの 100 ns）→ UNIX 時刻からのナノ秒。
fn ft_ns(ft: u64) -> i64 {
    if ft == 0 {
        return 0;
    }
    (ft as i64 - 116_444_736_000_000_000).saturating_mul(100)
}

fn row_of(name: String, path: PathBuf, m: &std::fs::Metadata) -> Row {
    let attrs = m.file_attributes();
    Row {
        name,
        path,
        dir: attrs & ATTR_DIRECTORY != 0,
        size: if attrs & ATTR_DIRECTORY != 0 {
            0
        } else {
            m.file_size()
        },
        mtime: ft_ns(m.last_write_time()),
        ctime: ft_ns(m.creation_time()),
        attrs,
        hidden: attrs & (ATTR_HIDDEN | ATTR_SYSTEM) != 0,
        ..Default::default()
    }
}

/// 列。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Col {
    Name,
    Folder,
    Modified,
    Type,
    Size,
    Created,
    Attr,
    Total,
    Free,
    Packed,
    Ratio,
    Method,
    Encrypted,
}

impl Col {
    fn title(self) -> &'static str {
        match self {
            Col::Name => "名前",
            Col::Folder => "フォルダ",
            Col::Modified => "更新日時",
            Col::Type => "種類",
            Col::Size => "サイズ",
            Col::Created => "作成日時",
            Col::Attr => "属性",
            Col::Total => "合計サイズ",
            Col::Free => "空き領域",
            Col::Packed => "圧縮後",
            Col::Ratio => "圧縮率",
            Col::Method => "方式",
            Col::Encrypted => "暗号化",
        }
    }

    fn width(self) -> i32 {
        match self {
            Col::Name => 280,
            Col::Folder => 260,
            Col::Modified | Col::Created => 130,
            Col::Type => 150,
            Col::Size | Col::Total | Col::Free | Col::Packed => 90,
            Col::Ratio => 60,
            Col::Method => 90,
            Col::Attr | Col::Encrypted => 60,
        }
    }

    fn right(self) -> bool {
        matches!(
            self,
            Col::Size | Col::Total | Col::Free | Col::Packed | Col::Ratio
        )
    }
}

/// ツリーの節。
struct Node {
    item: HTREEITEM,
    /// 開く場所（見出しは `None`）
    loc: Option<Loc>,
    /// 子のフォルダを読んだ
    loaded: bool,
}

/// 作業スレッドから届く一覧の一部。
struct Batch {
    generation: u64,
    rows: Vec<Row>,
    done: bool,
    error: Option<String>,
    dirs: usize,
    /// 空き領域（読み終えたとき）
    free: Option<u64>,
}

struct State {
    hwnd: HWND,
    buttons: Vec<(u16, HWND)>,
    address: HWND,
    filter: HWND,
    flat_btn: HWND,
    tree: HWND,
    list: HWND,
    dpi: u32,
    dirs: Dirs,
    st: BrowseState,
    loc: Loc,
    back: Vec<Loc>,
    forward: Vec<Loc>,
    flat: bool,
    rows: Vec<Row>,
    /// 表示する行（`rows` の番号。並べ替え・絞り込みの後）
    view: Vec<usize>,
    cols: Vec<Col>,
    sort: (Col, bool),
    generation: u64,
    cancel: Option<Arc<AtomicBool>>,
    loading: bool,
    scanned_dirs: usize,
    archive: Option<(PathBuf, Arc<Archive>)>,
    query: Option<Query>,
    query_error: Option<String>,
    props: HashMap<PathBuf, Props>,
    props_pending: HashSet<PathBuf>,
    icons: HashMap<String, i32>,
    types: HashMap<String, String>,
    folder_icon: i32,
    nodes: Vec<Node>,
    tree_gen: u64,
    dragging_splitter: bool,
    watch: Option<Arc<AtomicBool>>,
    /// 読み込みの後で選ぶもの
    reselect: Vec<PathBuf>,
    /// 読み直し: 新しい行が届くまで今の行を出しておく・読み終えたら戻す一番上の行
    replace_pending: bool,
    restore_top: Option<usize>,
    /// 空き領域
    free: Option<u64>,
    status: fn(&str),
    temp_root: PathBuf,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    STATE.with(|s| s.try_borrow_mut().ok()?.as_mut().map(f))
}

fn text_of(h: HWND) -> String {
    unsafe {
        let n = GetWindowTextLengthW(h).max(0) as usize;
        let mut buf = vec![0u16; n + 1];
        let got = GetWindowTextW(h, &mut buf) as usize;
        String::from_utf16_lossy(&buf[..got])
    }
}

fn set_text(h: HWND, s: &str) {
    unsafe {
        let _ = SetWindowTextW(h, &HSTRING::from(s));
    }
}

fn post<T>(hwnd: isize, msg: u32, wparam: usize, value: T) {
    let p = Box::into_raw(Box::new(value));
    unsafe {
        if PostMessageW(
            Some(HWND(hwnd as *mut _)),
            msg,
            WPARAM(wparam),
            LPARAM(p as isize),
        )
        .is_err()
        {
            drop(Box::from_raw(p));
        }
    }
}

/// `post` で送ったものを受け取る。
unsafe fn take<T>(lparam: LPARAM) -> Box<T> {
    unsafe { Box::from_raw(lparam.0 as *mut T) }
}

/// 「フォルダ」のタブの窓を作る（`parent` の子。初めは隠しておく）。
pub(super) fn create(
    parent: HWND,
    instance: HINSTANCE,
    font: HFONT,
    dpi: u32,
    dirs: Dirs,
    status: fn(&str),
) -> HWND {
    unsafe {
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(proc),
            hInstance: instance,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: windows::Win32::Graphics::Gdi::HBRUSH(
                (windows::Win32::Graphics::Gdi::COLOR_BTNFACE.0 + 1) as usize as *mut _,
            ),
            lpszClassName: CLASS,
            ..Default::default()
        };
        RegisterClassExW(&class);
        let hwnd = CreateWindowExW(
            WS_EX_ACCEPTFILES | WS_EX_CONTROLPARENT,
            CLASS,
            None,
            WS_CHILD | WS_CLIPCHILDREN,
            0,
            0,
            10,
            10,
            Some(parent),
            None,
            Some(instance),
            None,
        )
        .unwrap_or_default();
        let child =
            |class: PCWSTR, text: &str, style: WINDOW_STYLE, ex: WINDOW_EX_STYLE, id: u16| {
                let h = CreateWindowExW(
                    ex,
                    class,
                    &HSTRING::from(text),
                    WS_CHILD | WS_VISIBLE | style,
                    0,
                    0,
                    10,
                    10,
                    Some(hwnd),
                    Some(HMENU(id as usize as *mut _)),
                    Some(instance),
                    None,
                )
                .unwrap_or_default();
                SendMessageW(
                    h,
                    WM_SETFONT,
                    Some(WPARAM(font.0 as usize)),
                    Some(LPARAM(1)),
                );
                h
            };
        let button = |t: &str, id: u16| {
            child(
                w!("BUTTON"),
                t,
                WS_TABSTOP | WINDOW_STYLE(BS_PUSHBUTTON as u32),
                WINDOW_EX_STYLE(0),
                id,
            )
        };
        let mut buttons = Vec::new();
        for (id, t) in [
            (ID_BACK, "←"),
            (ID_FORWARD, "→"),
            (ID_UP, "↑"),
            (ID_REFRESH, "⟳"),
        ] {
            buttons.push((id, button(t, id)));
        }
        let address = child(
            w!("EDIT"),
            "",
            WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
            WS_EX_CLIENTEDGE,
            ID_ADDRESS,
        );
        shell::autocomplete(address);
        buttons.push((ID_GO, button("移動", ID_GO)));
        let filter = child(
            w!("EDIT"),
            "",
            WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
            WS_EX_CLIENTEDGE,
            ID_FILTER,
        );
        let cue = crate::util::wide(
            "絞り込み（Ctrl+F）: 名前の一部・*.jpg・re:正規表現・ext:pdf・size:>1MB・modified:2024・exif.model:EOS・id3.artist:名前",
        );
        SendMessageW(
            filter,
            EM_SETCUEBANNER,
            Some(WPARAM(1)),
            Some(LPARAM(cue.as_ptr() as isize)),
        );
        let flat_btn = child(
            w!("BUTTON"),
            "フラット",
            WS_TABSTOP | WINDOW_STYLE((BS_AUTOCHECKBOX | BS_PUSHLIKE) as u32),
            WINDOW_EX_STYLE(0),
            ID_FLAT,
        );
        for (id, t) in [
            (ID_NEW_FOLDER, "新しいフォルダ"),
            (ID_RENAME_MANY, "一括リネーム..."),
            (ID_VIEW, "表示 ▾"),
        ] {
            buttons.push((id, button(t, id)));
        }
        let tree = child(
            WC_TREEVIEWW,
            "",
            WS_TABSTOP
                | WINDOW_STYLE(
                    TVS_HASBUTTONS
                        | TVS_LINESATROOT
                        | TVS_SHOWSELALWAYS
                        | TVS_FULLROWSELECT
                        | TVS_INFOTIP,
                ),
            WS_EX_CLIENTEDGE,
            ID_TREE,
        );
        let list = child(
            WC_LISTVIEWW,
            "",
            WS_TABSTOP
                | WINDOW_STYLE(
                    LVS_REPORT
                        | LVS_SHOWSELALWAYS
                        | LVS_OWNERDATA
                        | LVS_EDITLABELS
                        | LVS_SHAREIMAGELISTS,
                ),
            WS_EX_CLIENTEDGE,
            ID_LIST,
        );
        let _ = SetWindowTheme(tree, w!("Explorer"), None);
        let _ = SetWindowTheme(list, w!("Explorer"), None);
        SendMessageW(
            list,
            LVM_SETEXTENDEDLISTVIEWSTYLE,
            Some(WPARAM(0)),
            Some(LPARAM(
                (LVS_EX_FULLROWSELECT
                    | LVS_EX_DOUBLEBUFFER
                    | LVS_EX_HEADERDRAGDROP
                    | LVS_EX_LABELTIP) as isize,
            )),
        );
        SendMessageW(
            tree,
            TVM_SETEXTENDEDSTYLE,
            Some(WPARAM(TVS_EX_DOUBLEBUFFER as usize)),
            Some(LPARAM(TVS_EX_DOUBLEBUFFER as isize)),
        );
        // システムのアイコン（小さいものと大きいもの）
        let mut info = SHFILEINFOW::default();
        let small = SHGetFileInfoW(
            w!("folder"),
            FILE_ATTRIBUTE_DIRECTORY,
            Some(&mut info),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_SYSICONINDEX | SHGFI_SMALLICON | SHGFI_USEFILEATTRIBUTES,
        );
        let folder_icon = info.iIcon;
        let large = SHGetFileInfoW(
            w!("folder"),
            FILE_ATTRIBUTE_DIRECTORY,
            Some(&mut info),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_SYSICONINDEX | SHGFI_LARGEICON | SHGFI_USEFILEATTRIBUTES,
        );
        if small != 0 {
            SendMessageW(
                list,
                LVM_SETIMAGELIST,
                Some(WPARAM(LVSIL_SMALL as usize)),
                Some(LPARAM(small as isize)),
            );
            SendMessageW(
                tree,
                TVM_SETIMAGELIST,
                Some(WPARAM(TVSIL_NORMAL as usize)),
                Some(LPARAM(small as isize)),
            );
        }
        if large != 0 {
            SendMessageW(
                list,
                LVM_SETIMAGELIST,
                Some(WPARAM(LVSIL_NORMAL as usize)),
                Some(LPARAM(large as isize)),
            );
        }
        DragAcceptFiles(hwnd, true);
        let st = BrowseState::load(&dirs.browse_file());
        let start = Loc::parse(&st.last).unwrap_or_else(|| {
            shell::known_folders()
                .into_iter()
                .find(|(n, _)| n == "ドキュメント")
                .map_or(Loc::Computer, |(_, p)| Loc::Folder(p))
        });
        let temp_root = std::env::temp_dir().join(format!("yyfilemanager-{}", std::process::id()));
        let s = State {
            hwnd,
            buttons,
            address,
            filter,
            flat_btn,
            tree,
            list,
            dpi,
            dirs,
            st,
            loc: start.clone(),
            back: Vec::new(),
            forward: Vec::new(),
            flat: false,
            rows: Vec::new(),
            view: Vec::new(),
            cols: Vec::new(),
            sort: (Col::Name, true),
            generation: 0,
            cancel: None,
            loading: false,
            scanned_dirs: 0,
            archive: None,
            query: None,
            query_error: None,
            props: HashMap::new(),
            props_pending: HashSet::new(),
            icons: HashMap::new(),
            types: HashMap::new(),
            folder_icon,
            nodes: Vec::new(),
            tree_gen: 0,
            dragging_splitter: false,
            watch: None,
            reselect: Vec::new(),
            replace_pending: false,
            restore_top: None,
            free: None,
            status,
            temp_root,
        };
        STATE.with(|c| *c.borrow_mut() = Some(s));
        with(|s| {
            s.apply_view();
            s.fill_tree();
        });
        go(start, false);
        hwnd
    }
}

/// 窓（fm の画面が置き場所を決める）。
pub(super) fn hwnd() -> HWND {
    with(|s| s.hwnd).unwrap_or_default()
}

/// 状態を保存し、一時フォルダを消す（終了するとき）。
pub(super) fn shutdown() {
    with(|s| {
        if let Some(c) = s.cancel.take() {
            c.store(true, Ordering::Relaxed);
        }
        if let Some(w) = s.watch.take() {
            w.store(true, Ordering::Relaxed);
        }
        s.save_state();
        let _ = std::fs::remove_dir_all(&s.temp_root);
    });
}

/// フォルダ（またはファイルのあるフォルダ）を開く。
pub(super) fn open_path(p: &Path) {
    let loc = if p.is_dir() {
        Loc::Folder(p.to_path_buf())
    } else {
        p.parent()
            .map_or(Loc::Computer, |q| Loc::Folder(q.to_path_buf()))
    };
    let sel = (!p.is_dir()).then(|| p.to_path_buf());
    go(loc, true);
    if let Some(sel) = sel {
        with(|s| s.reselect = vec![sel]);
    }
}

/// 絞り込みの欄へ。
pub(super) fn focus_filter() {
    if let Some(f) = with(|s| s.filter) {
        unsafe {
            let _ = SetFocus(Some(f));
            SendMessageW(f, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
        }
    }
}

impl State {
    fn d(&self, v: i32) -> i32 {
        v * self.dpi as i32 / 96
    }

    fn set_status(&self, s: &str) {
        (self.status)(s);
    }

    fn save_state(&mut self) {
        self.st.last = match &self.loc {
            Loc::Archive { file, .. } => file
                .parent()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default(),
            l => l.address(),
        };
        let _ = self.st.save(&self.dirs.browse_file());
    }

    fn in_archive(&self) -> bool {
        matches!(self.loc, Loc::Archive { .. })
    }

    fn layout(&self) {
        unsafe {
            let mut rc = RECT::default();
            let _ = GetClientRect(self.hwnd, &mut rc);
            let pad = self.d(4);
            let h = self.d(24);
            let mut x = pad;
            let mut y = pad;
            let btn = |id: u16| self.buttons.iter().find(|(i, _)| *i == id).map(|(_, h)| *h);
            for id in [ID_BACK, ID_FORWARD, ID_UP, ID_REFRESH] {
                if let Some(b) = btn(id) {
                    let _ = MoveWindow(b, x, y, self.d(30), h, true);
                    x += self.d(32);
                }
            }
            let go_w = self.d(50);
            let addr_w = (rc.right - x - pad - go_w - self.d(4)).max(self.d(100));
            let _ = MoveWindow(self.address, x, y, addr_w, h, true);
            if let Some(b) = btn(ID_GO) {
                let _ = MoveWindow(b, x + addr_w + self.d(4), y, go_w, h, true);
            }
            y += h + pad;
            let right_w = [
                (ID_FLAT, 80),
                (ID_NEW_FOLDER, 110),
                (ID_RENAME_MANY, 110),
                (ID_VIEW, 70),
            ];
            let total: i32 = right_w.iter().map(|(_, w)| self.d(*w) + self.d(4)).sum();
            let filter_w = (rc.right - 2 * pad - total).max(self.d(120));
            let _ = MoveWindow(self.filter, pad, y, filter_w, h, true);
            let mut x = pad + filter_w + self.d(4);
            for (id, w) in right_w {
                let hw = if id == ID_FLAT {
                    Some(self.flat_btn)
                } else {
                    btn(id)
                };
                if let Some(b) = hw {
                    let _ = MoveWindow(b, x, y, self.d(w), h, true);
                }
                x += self.d(w) + self.d(4);
            }
            y += h + pad;
            let tree_w = self.d(self.st.tree_width.clamp(80, 1200)).min(rc.right / 2);
            let body_h = (rc.bottom - y).max(0);
            let _ = MoveWindow(self.tree, 0, y, tree_w, body_h, true);
            let lx = tree_w + self.d(SPLITTER);
            let _ = MoveWindow(self.list, lx, y, (rc.right - lx).max(0), body_h, true);
        }
    }

    fn splitter_rect(&self) -> RECT {
        let mut rc = RECT::default();
        unsafe {
            let _ = GetWindowRect(self.tree, &mut rc);
            let mut a = POINT {
                x: rc.right,
                y: rc.top,
            };
            let mut b = POINT {
                x: rc.right,
                y: rc.bottom,
            };
            let _ = ScreenToClient(self.hwnd, &mut a);
            let _ = ScreenToClient(self.hwnd, &mut b);
            RECT {
                left: a.x,
                top: a.y,
                right: a.x + self.d(SPLITTER),
                bottom: b.y,
            }
        }
    }

    fn apply_view(&self) {
        let v = match self.st.view.as_str() {
            "list" => LV_VIEW_LIST,
            "icons" => LV_VIEW_ICON,
            _ => LV_VIEW_DETAILS,
        };
        unsafe {
            SendMessageW(self.list, LVM_SETVIEW, Some(WPARAM(v as usize)), None);
        }
    }

    // ---- ツリー ----

    fn insert_node(
        &mut self,
        parent: HTREEITEM,
        text: &str,
        loc: Option<Loc>,
        image: i32,
        children: bool,
    ) -> HTREEITEM {
        let mut t: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
        let idx = self.nodes.len();
        let ins = TVINSERTSTRUCTW {
            hParent: parent,
            hInsertAfter: TVI_LAST,
            Anonymous: TVINSERTSTRUCTW_0 {
                itemex: TVITEMEXW {
                    mask: TVIF_TEXT | TVIF_PARAM | TVIF_IMAGE | TVIF_SELECTEDIMAGE | TVIF_CHILDREN,
                    pszText: PWSTR(t.as_mut_ptr()),
                    lParam: LPARAM(idx as isize),
                    iImage: image,
                    iSelectedImage: image,
                    cChildren: TVITEMEXW_CHILDREN(children as i32),
                    ..Default::default()
                },
            },
        };
        let item = unsafe {
            HTREEITEM(
                SendMessageW(
                    self.tree,
                    TVM_INSERTITEMW,
                    None,
                    Some(LPARAM(&ins as *const _ as isize)),
                )
                .0,
            )
        };
        self.nodes.push(Node {
            item,
            loc,
            loaded: !children,
        });
        item
    }

    fn real_icon(&mut self, p: &Path) -> i32 {
        let key = format!("\u{1}{}", p.to_string_lossy().to_lowercase());
        if let Some(&i) = self.icons.get(&key) {
            return i;
        }
        let w = crate::util::wide(&p.to_string_lossy());
        let mut info = SHFILEINFOW::default();
        unsafe {
            SHGetFileInfoW(
                PCWSTR(w.as_ptr()),
                FILE_FLAGS_AND_ATTRIBUTES(0),
                Some(&mut info),
                std::mem::size_of::<SHFILEINFOW>() as u32,
                SHGFI_SYSICONINDEX | SHGFI_SMALLICON,
            );
        }
        self.icons.insert(key, info.iIcon);
        info.iIcon
    }

    fn fill_tree(&mut self) {
        unsafe {
            SendMessageW(self.tree, WM_SETREDRAW, Some(WPARAM(0)), None);
            SendMessageW(self.tree, TVM_DELETEITEM, None, Some(LPARAM(TVI_ROOT.0)));
        }
        self.nodes.clear();
        self.tree_gen += 1;
        let star = self.folder_icon;
        let quick = self.insert_node(TVI_ROOT, "よく使うフォルダ", None, star, true);
        if let Some(n) = self.nodes.last_mut() {
            n.loaded = true;
        }
        for (name, p) in shell::known_folders() {
            let icon = self.real_icon(&p);
            self.insert_node(quick, &name, Some(Loc::Folder(p)), icon, true);
        }
        for p in self.st.pinned.clone() {
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| p.to_string_lossy().into_owned());
            let icon = self.real_icon(&p);
            self.insert_node(
                quick,
                &format!("📌 {name}"),
                Some(Loc::Folder(p)),
                icon,
                true,
            );
        }
        let pc_icon = self.folder_icon;
        let pc = self.insert_node(TVI_ROOT, "PC", Some(Loc::Computer), pc_icon, true);
        if let Some(n) = self.nodes.last_mut() {
            n.loaded = true;
        }
        for d in drives() {
            let label = shell::display_name(&d);
            let icon = self.real_icon(&d);
            self.insert_node(pc, &label, Some(Loc::Folder(d)), icon, true);
        }
        unsafe {
            for item in [quick, pc] {
                SendMessageW(
                    self.tree,
                    TVM_EXPAND,
                    Some(WPARAM(TVE_EXPAND.0 as usize)),
                    Some(LPARAM(item.0)),
                );
            }
            SendMessageW(self.tree, WM_SETREDRAW, Some(WPARAM(1)), None);
            let _ = InvalidateRect(Some(self.tree), None, true);
        }
    }

    fn node_of(&self, item: HTREEITEM) -> Option<usize> {
        let mut it = TVITEMW {
            mask: TVIF_PARAM | TVIF_HANDLE,
            hItem: item,
            ..Default::default()
        };
        unsafe {
            SendMessageW(
                self.tree,
                TVM_GETITEMW,
                None,
                Some(LPARAM(&mut it as *mut _ as isize)),
            );
        }
        let i = it.lParam.0 as usize;
        (i < self.nodes.len() && self.nodes[i].item == item).then_some(i)
    }

    /// 子のフォルダを読む（作業スレッド）。
    fn load_children(&mut self, index: usize) {
        let Some(Node {
            loc: Some(Loc::Folder(p)),
            loaded: false,
            ..
        }) = self.nodes.get(index)
        else {
            return;
        };
        let p = p.clone();
        self.nodes[index].loaded = true;
        let hwnd = self.hwnd.0 as isize;
        let generation = self.tree_gen;
        let show_hidden = self.st.show_hidden;
        std::thread::spawn(move || {
            let mut dirs: Vec<(String, PathBuf)> = std::fs::read_dir(&p)
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|e| {
                    let m = e.metadata().ok()?;
                    let a = m.file_attributes();
                    (a & ATTR_DIRECTORY != 0
                        && (show_hidden || a & (ATTR_HIDDEN | ATTR_SYSTEM) == 0))
                        .then(|| (e.file_name().to_string_lossy().into_owned(), e.path()))
                })
                .collect();
            dirs.sort_by(|a, b| yy_config::workspace::natural_cmp(&a.0, &b.0));
            post(hwnd, WM_APP_TREE, index, (generation, dirs));
        });
    }

    fn on_tree_loaded(&mut self, index: usize, generation: u64, dirs: Vec<(String, PathBuf)>) {
        if generation != self.tree_gen || index >= self.nodes.len() {
            return;
        }
        let parent = self.nodes[index].item;
        if dirs.is_empty() {
            let mut it = TVITEMW {
                mask: TVIF_CHILDREN | TVIF_HANDLE,
                hItem: parent,
                cChildren: TVITEMEXW_CHILDREN(0),
                ..Default::default()
            };
            unsafe {
                SendMessageW(
                    self.tree,
                    TVM_SETITEMW,
                    None,
                    Some(LPARAM(&mut it as *mut _ as isize)),
                );
            }
            return;
        }
        let icon = self.folder_icon;
        for (name, p) in dirs {
            self.insert_node(parent, &name, Some(Loc::Folder(p)), icon, true);
        }
        unsafe {
            SendMessageW(
                self.tree,
                TVM_EXPAND,
                Some(WPARAM(TVE_EXPAND.0 as usize)),
                Some(LPARAM(parent.0)),
            );
        }
    }

    // ---- 一覧 ----

    fn columns_for(&self) -> Vec<Col> {
        match &self.loc {
            Loc::Computer => vec![Col::Name, Col::Type, Col::Total, Col::Free],
            Loc::Archive { .. } => {
                let mut v = vec![Col::Name];
                if self.flat {
                    v.push(Col::Folder);
                }
                v.extend([
                    Col::Size,
                    Col::Packed,
                    Col::Ratio,
                    Col::Method,
                    Col::Modified,
                    Col::Encrypted,
                ]);
                v
            }
            Loc::Folder(_) => {
                let mut v = vec![Col::Name];
                if self.flat {
                    v.push(Col::Folder);
                }
                v.extend([Col::Modified, Col::Type, Col::Size, Col::Created, Col::Attr]);
                v
            }
        }
    }

    fn set_columns(&mut self) {
        let cols = self.columns_for();
        if cols == self.cols {
            return;
        }
        unsafe {
            while SendMessageW(self.list, LVM_DELETECOLUMN, Some(WPARAM(0)), None).0 != 0 {}
            for (i, c) in cols.iter().enumerate() {
                let t = crate::util::wide(c.title());
                let col = LVCOLUMNW {
                    mask: LVCF_TEXT | LVCF_WIDTH | LVCF_FMT,
                    fmt: if c.right() { LVCFMT_RIGHT } else { LVCFMT_LEFT },
                    cx: self.d(c.width()),
                    pszText: PWSTR(t.as_ptr() as *mut _),
                    ..Default::default()
                };
                SendMessageW(
                    self.list,
                    LVM_INSERTCOLUMNW,
                    Some(WPARAM(i)),
                    Some(LPARAM(&col as *const _ as isize)),
                );
            }
        }
        if !cols.contains(&self.sort.0) {
            self.sort = (Col::Name, true);
        }
        self.cols = cols;
        self.show_sort_arrow();
    }

    fn show_sort_arrow(&self) {
        unsafe {
            let header = HWND(SendMessageW(self.list, LVM_GETHEADER, None, None).0 as *mut _);
            for (i, c) in self.cols.iter().enumerate() {
                let mut it = HDITEMW {
                    mask: HDI_FORMAT,
                    ..Default::default()
                };
                SendMessageW(
                    header,
                    HDM_GETITEMW,
                    Some(WPARAM(i)),
                    Some(LPARAM(&mut it as *mut _ as isize)),
                );
                let mut f = it.fmt.0 & !(HDF_SORTUP.0 | HDF_SORTDOWN.0);
                if *c == self.sort.0 {
                    f |= if self.sort.1 {
                        HDF_SORTUP.0
                    } else {
                        HDF_SORTDOWN.0
                    };
                }
                it.fmt = HEADER_CONTROL_FORMAT_FLAGS(f);
                SendMessageW(
                    header,
                    HDM_SETITEMW,
                    Some(WPARAM(i)),
                    Some(LPARAM(&it as *const _ as isize)),
                );
            }
        }
    }

    fn entry(&self, r: &Row) -> Option<&yy_files::archive::Entry> {
        let (_, a) = self.archive.as_ref()?;
        a.entries.get(r.entry?)
    }

    fn type_name(&mut self, r: &Row) -> String {
        if r.drive.is_some() {
            return "ローカル ディスク".into();
        }
        if r.dir {
            return "ファイル フォルダー".into();
        }
        let ext = r
            .name
            .rsplit_once('.')
            .map(|(_, e)| e.to_ascii_lowercase())
            .unwrap_or_default();
        if let Some(t) = self.types.get(&ext) {
            return t.clone();
        }
        let w = crate::util::wide(&format!("x.{ext}"));
        let mut info = SHFILEINFOW::default();
        unsafe {
            SHGetFileInfoW(
                PCWSTR(w.as_ptr()),
                FILE_ATTRIBUTE_NORMAL,
                Some(&mut info),
                std::mem::size_of::<SHFILEINFOW>() as u32,
                SHGFI_TYPENAME | SHGFI_USEFILEATTRIBUTES,
            );
        }
        let n = info.szTypeName.iter().position(|&c| c == 0).unwrap_or(0);
        let mut t = String::from_utf16_lossy(&info.szTypeName[..n]);
        if t.is_empty() {
            t = if ext.is_empty() {
                "ファイル".into()
            } else {
                format!("{} ファイル", ext.to_uppercase())
            };
        }
        self.types.insert(ext, t.clone());
        t
    }

    fn icon(&mut self, r: &Row) -> i32 {
        if r.drive.is_some() {
            return self.real_icon(&r.path.clone());
        }
        if r.dir {
            return self.folder_icon;
        }
        let ext = r
            .name
            .rsplit_once('.')
            .map(|(_, e)| e.to_ascii_lowercase())
            .unwrap_or_default();
        // 実行ファイル・ショートカットはファイルごとのアイコン
        if r.entry.is_none()
            && matches!(
                ext.as_str(),
                "exe" | "lnk" | "ico" | "url" | "cpl" | "msc" | "appref-ms"
            )
        {
            return self.real_icon(&r.path.clone());
        }
        if let Some(&i) = self.icons.get(&ext) {
            return i;
        }
        let w = crate::util::wide(&format!("x.{ext}"));
        let mut info = SHFILEINFOW::default();
        unsafe {
            SHGetFileInfoW(
                PCWSTR(w.as_ptr()),
                FILE_ATTRIBUTE_NORMAL,
                Some(&mut info),
                std::mem::size_of::<SHFILEINFOW>() as u32,
                SHGFI_SYSICONINDEX | SHGFI_SMALLICON | SHGFI_USEFILEATTRIBUTES,
            );
        }
        self.icons.insert(ext, info.iIcon);
        info.iIcon
    }

    fn display_name(&self, r: &Row) -> String {
        if r.drive.is_some() {
            return shell::display_name(&r.path);
        }
        if !self.st.show_extensions
            && !r.dir
            && let Some((stem, _)) = r.name.rsplit_once('.')
            && !stem.is_empty()
        {
            return stem.to_owned();
        }
        r.name.clone()
    }

    fn cell(&mut self, view_row: usize, col: usize) -> String {
        let Some(&i) = self.view.get(view_row) else {
            return String::new();
        };
        let Some(c) = self.cols.get(col).copied() else {
            return String::new();
        };
        let r = self.rows[i].clone();
        let size = |n: u64| crate::util::human_size(n);
        match c {
            Col::Name => self.display_name(&r),
            Col::Folder => r.folder.clone(),
            Col::Modified => match self.entry(&r) {
                Some(e) => match e.mtime {
                    Some(t) if e.mtime_local => yy_files::props::strftime(t, "%Y/%m/%d %H:%M"),
                    Some(t) => super::time_text(t.saturating_mul(1_000_000_000)),
                    None => String::new(),
                },
                None => super::time_text(r.mtime),
            },
            Col::Created => super::time_text(r.ctime),
            Col::Type => self.type_name(&r),
            Col::Size => {
                if r.dir {
                    String::new()
                } else {
                    size(r.size)
                }
            }
            Col::Attr => {
                let mut s = String::new();
                for (bit, ch) in [
                    (ATTR_READONLY, 'R'),
                    (ATTR_HIDDEN, 'H'),
                    (ATTR_SYSTEM, 'S'),
                    (ATTR_ARCHIVE, 'A'),
                    (ATTR_COMPRESSED, 'C'),
                    (ATTR_ENCRYPTED, 'E'),
                    (ATTR_REPARSE, 'L'),
                ] {
                    if r.attrs & bit != 0 {
                        s.push(ch);
                    }
                }
                s
            }
            Col::Total => r.drive.map(|(t, _)| size(t)).unwrap_or_default(),
            Col::Free => r.drive.map(|(_, f)| size(f)).unwrap_or_default(),
            Col::Packed => self
                .entry(&r)
                .and_then(|e| e.packed)
                .map(size)
                .unwrap_or_default(),
            Col::Ratio => match self.entry(&r) {
                Some(e) if e.size > 0 && e.packed.is_some() => {
                    let p = e.packed.unwrap_or(0) as f64 / e.size as f64;
                    format!("{:.0}%", (1.0 - p).max(0.0) * 100.0)
                }
                _ => String::new(),
            },
            Col::Method => self.entry(&r).map(|e| e.method.clone()).unwrap_or_default(),
            Col::Encrypted => {
                if self.entry(&r).is_some_and(|e| e.encrypted) {
                    "🔒".into()
                } else {
                    String::new()
                }
            }
        }
    }

    /// 2 つの行を今の列で比べる（フォルダが先）。
    fn compare(&self, a: &Row, b: &Row) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        if a.dir != b.dir && a.drive.is_none() {
            return if a.dir {
                Ordering::Less
            } else {
                Ordering::Greater
            };
        }
        let natural = |x: &str, y: &str| yy_config::workspace::natural_cmp(x, y);
        let (col, asc) = self.sort;
        let ea = || self.entry(a);
        let eb = || self.entry(b);
        let o = match col {
            Col::Name => natural(&a.name, &b.name),
            Col::Folder => natural(&a.folder, &b.folder).then_with(|| natural(&a.name, &b.name)),
            Col::Modified => match (ea(), eb()) {
                (Some(x), Some(y)) => x.mtime.cmp(&y.mtime),
                _ => a.mtime.cmp(&b.mtime),
            },
            Col::Created => a.ctime.cmp(&b.ctime),
            Col::Type => {
                let ext = |r: &Row| {
                    r.name
                        .rsplit_once('.')
                        .map(|(_, e)| e.to_lowercase())
                        .unwrap_or_default()
                };
                ext(a).cmp(&ext(b)).then_with(|| natural(&a.name, &b.name))
            }
            Col::Size => a.size.cmp(&b.size),
            Col::Attr => a.attrs.cmp(&b.attrs),
            Col::Total => a.drive.map(|d| d.0).cmp(&b.drive.map(|d| d.0)),
            Col::Free => a.drive.map(|d| d.1).cmp(&b.drive.map(|d| d.1)),
            Col::Packed => ea()
                .and_then(|e| e.packed)
                .cmp(&eb().and_then(|e| e.packed)),
            Col::Ratio => {
                let r = |e: Option<&yy_files::archive::Entry>| {
                    e.and_then(|e| Some((e.packed? as f64 / e.size.max(1) as f64 * 1000.0) as u64))
                };
                r(eb()).cmp(&r(ea()))
            }
            Col::Method => ea()
                .map(|e| e.method.clone())
                .cmp(&eb().map(|e| e.method.clone())),
            Col::Encrypted => ea().map(|e| e.encrypted).cmp(&eb().map(|e| e.encrypted)),
        };
        let o = o.then_with(|| natural(&a.name, &b.name));
        if asc { o } else { o.reverse() }
    }

    /// 行が絞り込みに合うか（メタデータが要るのにまだ読んでいなければ `None`）。
    fn passes(&self, r: &Row) -> Option<bool> {
        if r.hidden && !self.st.show_hidden {
            return Some(false);
        }
        if self.flat && r.dir && !self.st.flat_folders {
            return Some(false);
        }
        let Some(q) = &self.query else {
            return Some(true);
        };
        let rel = if r.folder.is_empty() {
            r.name.clone()
        } else {
            format!("{}/{}", r.folder.replace('\\', "/"), r.name)
        };
        let fe = yy_files::FileEntry {
            rel,
            meta: yy_files::Meta {
                size: r.size,
                mtime: r.mtime,
                ctime: r.ctime,
                dir: r.dir,
                readonly: r.attrs & ATTR_READONLY != 0,
                hidden: r.attrs & ATTR_HIDDEN != 0,
                ..Default::default()
            },
        };
        if !q.matches(&fe) {
            return Some(false);
        }
        if q.meta.is_empty() {
            return Some(true);
        }
        if r.dir || r.entry.is_some() || !yy_files::props::has_props(&r.name) {
            return Some(false);
        }
        let props = self.props.get(&r.path)?;
        Some(q.matches_meta(props, super::tz_offset()))
    }

    /// 並べ替えと絞り込みをやり直す。
    fn rebuild_view(&mut self) {
        let selected = self.selected_paths();
        let mut need: Vec<PathBuf> = Vec::new();
        let mut view = Vec::with_capacity(self.rows.len());
        for (i, r) in self.rows.iter().enumerate() {
            match self.passes(r) {
                Some(true) => view.push(i),
                Some(false) => {}
                None => {
                    if !self.props_pending.contains(&r.path) {
                        need.push(r.path.clone());
                    }
                }
            }
        }
        let rows = &self.rows;
        view.sort_by(|&a, &b| self.compare(&rows[a], &rows[b]));
        self.view = view;
        unsafe {
            SendMessageW(
                self.list,
                LVM_SETITEMCOUNT,
                Some(WPARAM(self.view.len())),
                Some(LPARAM(LVSICF_NOSCROLL as isize)),
            );
            let _ = InvalidateRect(Some(self.list), None, false);
        }
        self.select_paths(&selected, false);
        if !need.is_empty() {
            self.read_props(need);
        }
        self.update_status();
    }

    /// 足した行だけを表示に足す（読み込みの途中。並べ替えは終わってから）。
    fn append_view(&mut self, from: usize) {
        for i in from..self.rows.len() {
            if self.passes(&self.rows[i]) == Some(true) {
                self.view.push(i);
            }
        }
        unsafe {
            SendMessageW(
                self.list,
                LVM_SETITEMCOUNT,
                Some(WPARAM(self.view.len())),
                Some(LPARAM((LVSICF_NOSCROLL | LVSICF_NOINVALIDATEALL) as isize)),
            );
            let _ = InvalidateRect(Some(self.list), None, false);
        }
    }

    fn read_props(&mut self, paths: Vec<PathBuf>) {
        for p in &paths {
            self.props_pending.insert(p.clone());
        }
        let hwnd = self.hwnd.0 as isize;
        let cancel = self.cancel.clone().unwrap_or_default();
        std::thread::spawn(move || {
            let mut batch = Vec::new();
            let mut last = std::time::Instant::now();
            for p in paths {
                if cancel.load(Ordering::Relaxed) {
                    return;
                }
                let props = yy_files::props::read(&p);
                batch.push((p, props));
                if last.elapsed().as_millis() > 300 {
                    post(hwnd, WM_APP_PROPS, 0, std::mem::take(&mut batch));
                    last = std::time::Instant::now();
                }
            }
            post(hwnd, WM_APP_PROPS, 0, batch);
        });
    }

    fn selected(&self) -> Vec<usize> {
        let mut out = Vec::new();
        let mut i = -1isize;
        loop {
            i = unsafe {
                SendMessageW(
                    self.list,
                    LVM_GETNEXTITEM,
                    Some(WPARAM(i as usize)),
                    Some(LPARAM(LVNI_SELECTED as isize)),
                )
                .0
            };
            if i < 0 {
                break;
            }
            if let Some(&r) = self.view.get(i as usize) {
                out.push(r);
            }
        }
        out
    }

    fn selected_paths(&self) -> Vec<PathBuf> {
        self.selected()
            .into_iter()
            .filter(|&i| self.rows[i].entry.is_none())
            .map(|i| self.rows[i].path.clone())
            .collect()
    }

    fn select_paths(&self, paths: &[PathBuf], focus: bool) {
        if paths.is_empty() {
            return;
        }
        let want: HashSet<String> = paths
            .iter()
            .map(|p| p.to_string_lossy().to_lowercase())
            .collect();
        let mut first = None;
        unsafe {
            let none = LVITEMW {
                stateMask: LVIS_SELECTED,
                state: LIST_VIEW_ITEM_STATE_FLAGS(0),
                ..Default::default()
            };
            SendMessageW(
                self.list,
                LVM_SETITEMSTATE,
                Some(WPARAM(usize::MAX)),
                Some(LPARAM(&none as *const _ as isize)),
            );
            for (vi, &ri) in self.view.iter().enumerate() {
                if want.contains(&self.rows[ri].path.to_string_lossy().to_lowercase()) {
                    let mut it = LVITEMW {
                        stateMask: LVIS_SELECTED,
                        state: LVIS_SELECTED,
                        ..Default::default()
                    };
                    if first.is_none() {
                        first = Some(vi);
                        it.stateMask = LIST_VIEW_ITEM_STATE_FLAGS(it.stateMask.0 | LVIS_FOCUSED.0);
                        it.state = LIST_VIEW_ITEM_STATE_FLAGS(it.state.0 | LVIS_FOCUSED.0);
                    }
                    SendMessageW(
                        self.list,
                        LVM_SETITEMSTATE,
                        Some(WPARAM(vi)),
                        Some(LPARAM(&it as *const _ as isize)),
                    );
                }
            }
            if let Some(f) = first
                && focus
            {
                SendMessageW(
                    self.list,
                    LVM_ENSUREVISIBLE,
                    Some(WPARAM(f)),
                    Some(LPARAM(0)),
                );
            }
        }
    }

    fn update_status(&self) {
        let sel = self.selected();
        let mut s = match &self.loc {
            Loc::Computer => format!("{} 個のドライブ", self.view.len()),
            _ => {
                let files = self.view.iter().filter(|&&i| !self.rows[i].dir).count();
                let dirs = self.view.len() - files;
                let mut s = if self.flat {
                    format!("フラット: {files} 個のファイル")
                } else {
                    format!(
                        "{} 個の項目（フォルダ {dirs}・ファイル {files}）",
                        self.view.len()
                    )
                };
                if self.view.len() != self.rows.len() {
                    s.push_str(&format!("／全体 {}", self.rows.len()));
                }
                s
            }
        };
        if !sel.is_empty() {
            let bytes: u64 = sel.iter().map(|&i| self.rows[i].size).sum();
            s.push_str(&format!(
                "　{} 個を選択（{}）",
                sel.len(),
                crate::util::human_size(bytes)
            ));
        }
        if self.loading {
            s.push_str(&format!(
                "　読み込み中…（{} フォルダ）Esc で止める",
                self.scanned_dirs
            ));
        }
        if let Some(e) = &self.query_error {
            s.push_str(&format!("　絞り込みの誤り: {e}"));
        } else if !self.props_pending.is_empty() {
            s.push_str(&format!(
                "　メタデータを読んでいます（残り {}）",
                self.props_pending.len()
            ));
        }
        if let Loc::Archive { file, .. } = &self.loc {
            if let Some((_, a)) = &self.archive {
                s.push_str(&format!("　{}（{}）", file.display(), a.kind.label()));
            }
        } else if let (Loc::Folder(_), Some(free)) = (&self.loc, self.free) {
            s.push_str(&format!("　空き {}", crate::util::human_size(free)));
        }
        self.set_status(&s);
    }

    fn update_buttons(&self) {
        unsafe {
            for (id, h) in &self.buttons {
                let on = match *id {
                    ID_BACK => !self.back.is_empty(),
                    ID_FORWARD => !self.forward.is_empty(),
                    ID_UP => self.loc.parent().is_some(),
                    ID_NEW_FOLDER => matches!(self.loc, Loc::Folder(_)),
                    ID_RENAME_MANY => matches!(self.loc, Loc::Folder(_)),
                    _ => true,
                };
                let _ = EnableWindow(*h, on);
            }
            let _ = EnableWindow(self.flat_btn, !matches!(self.loc, Loc::Computer));
            SendMessageW(
                self.flat_btn,
                BM_SETCHECK,
                Some(WPARAM(self.flat as usize)),
                None,
            );
        }
    }

    /// 圧縮ファイルの中の行を作る。
    fn archive_rows(&mut self) {
        let Loc::Archive { file, inner } = &self.loc else {
            return;
        };
        let Some((_, a)) = self.archive.as_ref() else {
            return;
        };
        let idx = if self.flat {
            a.under(inner)
        } else {
            a.children(inner)
        };
        let prefix = if inner.is_empty() {
            String::new()
        } else {
            format!("{inner}/")
        };
        self.rows = idx
            .into_iter()
            .map(|i| {
                let e = &a.entries[i];
                let folder = e
                    .dir_part()
                    .strip_prefix(prefix.trim_end_matches('/'))
                    .unwrap_or(e.dir_part());
                Row {
                    name: e.name().to_owned(),
                    folder: folder.trim_start_matches('/').replace('/', "\\"),
                    path: file.clone(),
                    entry: Some(i),
                    dir: e.dir,
                    size: e.size,
                    mtime: e.mtime.map_or(0, |t| t.saturating_mul(1_000_000_000)),
                    ..Default::default()
                }
            })
            .collect();
    }
}

/// ドライブ（`C:\` など）。
fn drives() -> Vec<PathBuf> {
    let mask = unsafe { GetLogicalDrives() };
    (0..26)
        .filter(|i| mask & (1 << i) != 0)
        .map(|i| PathBuf::from(format!("{}:\\", (b'A' + i as u8) as char)))
        .collect()
}

/// 合計と空き（分からなければ `None`）。
fn disk_space(p: &Path) -> Option<(u64, u64)> {
    let w = crate::util::wide(&p.to_string_lossy());
    let (mut free, mut total) = (0u64, 0u64);
    unsafe {
        GetDiskFreeSpaceExW(PCWSTR(w.as_ptr()), Some(&mut free), Some(&mut total), None).ok()?
    };
    Some((total, free))
}

// ---- 移動・読み込み ----

/// 場所へ移る（`push` なら戻るの履歴に積む）。
fn go(loc: Loc, push: bool) {
    let Some(()) = with(|s| {
        if push && s.loc != loc {
            let old = s.loc.clone();
            s.back.push(old);
            s.forward.clear();
        }
        s.loc = loc.clone();
        if matches!(loc, Loc::Computer) {
            s.flat = false;
        }
        set_text(s.address, &loc.address());
        s.update_buttons();
        s.save_state();
    }) else {
        return;
    };
    load();
}

/// 今の場所を読み込む。
fn load() {
    load_with(false);
}

/// `reload` なら、新しい行が届くまで今の一覧を出しておく（見張りで読み直すときにちらつかない）。
fn load_with(reload: bool) {
    with(|s| {
        if let Some(c) = s.cancel.take() {
            c.store(true, Ordering::Relaxed);
        }
        if let Some(w) = s.watch.take() {
            w.store(true, Ordering::Relaxed);
        }
        s.generation += 1;
        s.props_pending.clear();
        s.scanned_dirs = 0;
        s.replace_pending = reload && !s.rows.is_empty();
        if !s.replace_pending {
            s.rows.clear();
            s.view.clear();
            s.restore_top = None;
            s.free = None;
            s.set_columns();
            unsafe {
                SendMessageW(s.list, LVM_SETITEMCOUNT, Some(WPARAM(0)), None);
            }
        }
        let cancel = Arc::new(AtomicBool::new(false));
        s.cancel = Some(cancel.clone());
        let hwnd = s.hwnd.0 as isize;
        let generation = s.generation;
        match s.loc.clone() {
            Loc::Computer => {
                s.loading = true;
                std::thread::spawn(move || {
                    let rows = drives()
                        .into_iter()
                        .map(|d| Row {
                            name: d.to_string_lossy().into_owned(),
                            drive: Some(disk_space(&d).unwrap_or((0, 0))),
                            dir: true,
                            path: d,
                            ..Default::default()
                        })
                        .collect();
                    post(
                        hwnd,
                        WM_APP_ROWS,
                        0,
                        Batch {
                            generation,
                            rows,
                            done: true,
                            error: None,
                            dirs: 0,
                            free: None,
                        },
                    );
                });
            }
            Loc::Folder(p) => {
                s.loading = true;
                let flat = s.flat;
                std::thread::spawn(move || list_folder(hwnd, generation, p, flat, cancel));
                if !s.flat {
                    s.watch = Some(watch(hwnd, generation, s.loc.clone()));
                }
            }
            Loc::Archive { file, .. } => {
                let cached = s.archive.as_ref().is_some_and(|(f, _)| *f == file);
                if cached {
                    s.archive_rows();
                    s.loading = false;
                    s.rebuild_view();
                } else {
                    s.loading = true;
                    s.archive = None;
                    std::thread::spawn(move || {
                        let c2 = cancel.clone();
                        let r = Archive::open(&file, &move || c2.load(Ordering::Relaxed));
                        post(
                            hwnd,
                            WM_APP_ARCHIVE,
                            0,
                            (generation, file, r.map(Arc::new).map_err(|e| e.to_string())),
                        );
                    });
                }
            }
        }
        s.update_status();
    });
}

/// フォルダを読む（作業スレッド）。`flat` なら下のフォルダもすべて。
fn list_folder(hwnd: isize, generation: u64, root: PathBuf, flat: bool, cancel: Arc<AtomicBool>) {
    let free = disk_space(&root).map(|(_, f)| f);
    let mut stack: Vec<(PathBuf, String, bool)> = vec![(root, String::new(), false)];
    let mut batch: Vec<Row> = Vec::new();
    let mut last = std::time::Instant::now();
    let mut dirs = 0usize;
    let mut first_error = None;
    while let Some((dir, rel, hidden)) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        dirs += 1;
        let rd = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(e) => {
                if dirs == 1 {
                    first_error = Some(format!("{}: {e}", dir.display()));
                }
                continue;
            }
        };
        for e in rd.flatten() {
            let Ok(m) = e.metadata() else { continue };
            let name = e.file_name().to_string_lossy().into_owned();
            let mut row = row_of(name.clone(), e.path(), &m);
            row.folder = rel.clone();
            row.hidden |= hidden;
            if flat && row.dir && row.attrs & ATTR_REPARSE == 0 {
                let child_rel = if rel.is_empty() {
                    name
                } else {
                    format!("{rel}\\{name}")
                };
                stack.push((row.path.clone(), child_rel, row.hidden));
            }
            batch.push(row);
        }
        if batch.len() >= 5000 || last.elapsed().as_millis() > 200 {
            post(
                hwnd,
                WM_APP_ROWS,
                0,
                Batch {
                    generation,
                    rows: std::mem::take(&mut batch),
                    done: false,
                    error: None,
                    dirs,
                    free: None,
                },
            );
            last = std::time::Instant::now();
        }
    }
    post(
        hwnd,
        WM_APP_ROWS,
        0,
        Batch {
            generation,
            rows: batch,
            done: true,
            error: first_error,
            dirs,
            free,
        },
    );
}

/// フォルダの変化を見張る（変わったら `WM_APP_CHANGED`）。止める印を返す。
fn watch(hwnd: isize, generation: u64, loc: Loc) -> Arc<AtomicBool> {
    use windows::Win32::Storage::FileSystem::{
        FILE_NOTIFY_CHANGE_ATTRIBUTES, FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME,
        FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE, FindCloseChangeNotification,
        FindFirstChangeNotificationW, FindNextChangeNotification,
    };
    let stop = Arc::new(AtomicBool::new(false));
    let s2 = stop.clone();
    let Loc::Folder(p) = loc else { return stop };
    std::thread::spawn(move || unsafe {
        let w = HSTRING::from(p.as_os_str());
        let Ok(h) = FindFirstChangeNotificationW(
            &w,
            false,
            FILE_NOTIFY_CHANGE_FILE_NAME
                | FILE_NOTIFY_CHANGE_DIR_NAME
                | FILE_NOTIFY_CHANGE_SIZE
                | FILE_NOTIFY_CHANGE_LAST_WRITE
                | FILE_NOTIFY_CHANGE_ATTRIBUTES,
        ) else {
            return;
        };
        while !s2.load(Ordering::Relaxed) {
            if WaitForSingleObject(h, 500).0 == 0 {
                if s2.load(Ordering::Relaxed) {
                    break;
                }
                let _ = PostMessageW(
                    Some(HWND(hwnd as *mut _)),
                    WM_APP_CHANGED,
                    WPARAM(generation as usize),
                    LPARAM(0),
                );
                if FindNextChangeNotification(h).is_err() {
                    break;
                }
            }
        }
        let _ = FindCloseChangeNotification(h);
    });
    stop
}

fn on_rows(b: Batch) {
    with(|s| {
        if b.generation != s.generation {
            return;
        }
        s.scanned_dirs = b.dirs;
        if s.replace_pending {
            s.replace_pending = false;
            s.rows.clear();
            s.view.clear();
        }
        let from = s.rows.len();
        s.rows.extend(b.rows);
        if b.done {
            s.loading = false;
            s.free = b.free.or(s.free);
            s.rebuild_view();
            let sel = std::mem::take(&mut s.reselect);
            s.select_paths(&sel, sel.len() == 1);
            if let Some(top) = s.restore_top.take() {
                // 一番上の行を戻す（下の端を見せてから上の端を見せる）
                unsafe {
                    let per = SendMessageW(s.list, LVM_GETCOUNTPERPAGE, None, None)
                        .0
                        .max(1) as usize;
                    let last = (top + per - 1).min(s.view.len().saturating_sub(1));
                    SendMessageW(
                        s.list,
                        LVM_ENSUREVISIBLE,
                        Some(WPARAM(last)),
                        Some(LPARAM(0)),
                    );
                    SendMessageW(
                        s.list,
                        LVM_ENSUREVISIBLE,
                        Some(WPARAM(top.min(last))),
                        Some(LPARAM(0)),
                    );
                }
            }
            if let Some(e) = b.error {
                s.set_status(&e);
            }
        } else {
            s.append_view(from);
            s.update_status();
        }
    });
}

fn back() {
    let loc = with(|s| {
        let l = s.back.pop()?;
        let cur = s.loc.clone();
        s.forward.push(cur);
        Some(l)
    })
    .flatten();
    if let Some(l) = loc {
        go(l, false);
    }
}

fn forward() {
    let loc = with(|s| {
        let l = s.forward.pop()?;
        let cur = s.loc.clone();
        s.back.push(cur);
        Some(l)
    })
    .flatten();
    if let Some(l) = loc {
        go(l, false);
    }
}

fn up() {
    let (loc, child) = with(|s| {
        let child = match &s.loc {
            Loc::Folder(p) => Some(p.clone()),
            Loc::Archive { file, inner } if inner.is_empty() => Some(file.clone()),
            _ => None,
        };
        (s.loc.parent(), child)
    })
    .unwrap_or((None, None));
    if let Some(l) = loc {
        go(l, true);
        if let Some(c) = child {
            with(|s| s.reselect = vec![c]);
        }
    }
}

fn toggle_flat() {
    with(|s| {
        if matches!(s.loc, Loc::Computer) {
            return;
        }
        s.flat = !s.flat;
        s.update_buttons();
    });
    load();
}

fn go_address() {
    let Some((text, owner)) = with(|s| (text_of(s.address), s.hwnd)) else {
        return;
    };
    match Loc::parse(&text) {
        Some(l) => {
            go(l, true);
            if let Some(list) = with(|s| s.list) {
                unsafe {
                    let _ = SetFocus(Some(list));
                }
            }
        }
        None => error_box(owner, &format!("「{text}」が見つかりません。")),
    }
}

// ---- 開く ----

fn open_rows(rows: &[usize]) {
    let Some(items) = with(|s| rows.iter().map(|&i| s.rows[i].clone()).collect::<Vec<_>>()) else {
        return;
    };
    if items.len() == 1 || items.iter().all(|r| !r.dir) {
        // 1 つならフォルダにも入る。複数のファイルはそれぞれを開く
    } else {
        return;
    }
    for r in items.iter().take(if items.len() == 1 { 1 } else { 16 }) {
        open_row(r);
    }
}

fn open_row(r: &Row) {
    if let Some(i) = r.entry {
        let Some((dir, archive_entry)) = with(|s| {
            let (_, a) = s.archive.as_ref()?;
            Some((a.entries[i].dir, a.entries[i].path.clone()))
        })
        .flatten() else {
            return;
        };
        if dir {
            go(
                Loc::Archive {
                    file: r.path.clone(),
                    inner: archive_entry,
                },
                true,
            );
        } else {
            extract_and_open(i, Kind::of(&r.name).is_some());
        }
        return;
    }
    if r.dir {
        go(Loc::Folder(r.path.clone()), true);
        return;
    }
    if Kind::of(&r.name).is_some() {
        go(
            Loc::Archive {
                file: r.path.clone(),
                inner: String::new(),
            },
            true,
        );
        return;
    }
    let w = HSTRING::from(r.path.as_os_str());
    unsafe {
        windows::Win32::UI::Shell::ShellExecuteW(None, w!("open"), &w, None, None, SW_SHOWNORMAL);
    }
}

/// 圧縮ファイルの中の 1 項目を一時フォルダに展開し、開く（`enter` なら中へ入る）。
fn extract_and_open(index: usize, enter: bool) {
    let Some((hwnd, file, archive, temp)) = with(|s| {
        let (f, a) = s.archive.clone()?;
        let n = s
            .temp_root
            .join(&blake3::hash(f.to_string_lossy().as_bytes()).to_hex()[..12]);
        s.set_status("展開しています…");
        Some((s.hwnd.0 as isize, f, a, n))
    })
    .flatten() else {
        return;
    };
    std::thread::spawn(move || {
        let e = &archive.entries[index];
        let dir = temp.join(e.dir_part().replace('/', "\\"));
        let r = archive
            .extract_to_dir(&file, &[index], &dir, e.dir_part(), &|| false)
            .map(|_| dir.join(e.name()))
            .map_err(|err| err.to_string());
        post(hwnd, WM_APP_EXTRACTED, enter as usize, r);
    });
}

fn on_extracted(enter: bool, r: Result<PathBuf, String>) {
    match r {
        Ok(p) if enter => go(
            Loc::Archive {
                file: p,
                inner: String::new(),
            },
            true,
        ),
        Ok(p) => {
            let w = HSTRING::from(p.as_os_str());
            unsafe {
                windows::Win32::UI::Shell::ShellExecuteW(
                    None,
                    w!("open"),
                    &w,
                    None,
                    None,
                    SW_SHOWNORMAL,
                );
            }
            with(|s| s.update_status());
        }
        Err(e) => {
            let owner = hwnd();
            error_box(owner, &format!("展開できませんでした。\n{e}"));
        }
    }
}

/// 選んだ圧縮ファイルの中の項目を `dest` へ展開する（フォルダはその中ごと）。
fn extract_selected(dest: &Path) -> Result<u64, String> {
    let (file, archive, indices, strip) = with(|s| {
        let (f, a) = s.archive.clone().ok_or("圧縮ファイルを読んでいません")?;
        let Loc::Archive { inner, .. } = &s.loc else {
            return Err("圧縮ファイルの中ではありません");
        };
        let mut idx = Vec::new();
        for r in s.selected() {
            if let Some(i) = s.rows[r].entry {
                idx.push(i);
                if a.entries[i].dir {
                    idx.extend(a.under(&a.entries[i].path));
                }
            }
        }
        Ok((f, a, idx, inner.clone()))
    })
    .ok_or("")?
    .map_err(str::to_owned)?;
    if indices.is_empty() {
        return Err("展開するものを選んでください".into());
    }
    archive
        .extract_to_dir(&file, &indices, dest, &strip, &|| false)
        .map_err(|e| e.to_string())
}

// ---- 操作 ----

fn owner() -> HWND {
    hwnd()
}

/// 今の場所のフォルダ（圧縮ファイルの中・PC なら `None`）。
fn current_folder() -> Option<PathBuf> {
    with(|s| match &s.loc {
        Loc::Folder(p) => Some(p.clone()),
        _ => None,
    })
    .flatten()
}

fn reload_soon() {
    with(|s| unsafe {
        SetTimer(Some(s.hwnd), TIMER_RELOAD, 250, None);
    });
}

fn reload_keep_selection() {
    with(|s| {
        s.reselect = s.selected_paths();
        s.restore_top =
            Some(unsafe { SendMessageW(s.list, LVM_GETTOPINDEX, None, None).0.max(0) as usize });
    });
    load_with(true);
}

fn do_file_op(op: Op, paths: Vec<PathBuf>, dest: Option<PathBuf>) {
    if paths.is_empty() {
        return;
    }
    let o = owner();
    if let Err(e) = shell::file_op(o, op, &paths, dest.as_deref()) {
        error_box(o, &e);
    }
    reload_keep_selection();
}

fn delete_selected(permanent: bool) {
    let Some((paths, in_archive)) = with(|s| (s.selected_paths(), s.in_archive())) else {
        return;
    };
    if in_archive {
        error_box(owner(), "圧縮ファイルの中は書き換えられません。");
        return;
    }
    // PC（ドライブの一覧）では消さない
    if current_folder().is_none() {
        return;
    }
    do_file_op(
        if permanent { Op::Delete } else { Op::Recycle },
        paths,
        None,
    );
}

fn copy_selected(cut: bool) {
    let Some((paths, in_archive)) = with(|s| (s.selected_paths(), s.in_archive())) else {
        return;
    };
    if in_archive {
        if cut {
            error_box(
                owner(),
                "圧縮ファイルの中は書き換えられません。「展開してコピー...」を使ってください。",
            );
            return;
        }
        // 一時フォルダに展開してからクリップボードへ
        if let Some(paths) = extract_selection_to_temp() {
            shell::set_clipboard_files(owner(), &paths, false);
        }
        return;
    }
    if !paths.is_empty() {
        shell::set_clipboard_files(owner(), &paths, cut);
        with(|s| {
            s.set_status(&format!(
                "{} 個を{}しました",
                paths.len(),
                if cut { "切り取り" } else { "コピー" }
            ))
        });
    }
}

fn paste(into: Option<PathBuf>) {
    let o = owner();
    let Some(dest) = into.or_else(current_folder) else {
        error_box(o, "ここには貼り付けられません。");
        return;
    };
    let Some((paths, cut)) = shell::clipboard_files(o) else {
        return;
    };
    let op = if cut { Op::Move } else { Op::Copy };
    match shell::file_op(o, op, &paths, Some(&dest)) {
        Ok(_) => {
            if cut {
                shell::clear_clipboard(o);
            }
        }
        Err(e) => error_box(o, &e),
    }
    let names: Vec<PathBuf> = paths
        .iter()
        .filter_map(|p| p.file_name().map(|n| dest.join(n)))
        .collect();
    with(|s| s.reselect = names);
    load();
}

fn new_folder() {
    let Some(dir) = current_folder() else { return };
    let mut name = "新しいフォルダー".to_owned();
    let mut n = 2;
    while dir.join(&name).exists() {
        name = format!("新しいフォルダー ({n})");
        n += 1;
    }
    let p = dir.join(&name);
    if let Err(e) = std::fs::create_dir(&p) {
        error_box(owner(), &format!("{}\n{e}", p.display()));
        return;
    }
    // 一覧に足して、名前の変更を始める
    let vi = with(|s| {
        if let Ok(m) = std::fs::metadata(&p) {
            let mut r = row_of(name.clone(), p.clone(), &m);
            r.folder = String::new();
            s.rows.push(r);
            s.rebuild_view();
            s.select_paths(std::slice::from_ref(&p), true);
            let ri = s.rows.len() - 1;
            s.view.iter().position(|&v| v == ri)
        } else {
            None
        }
    })
    .flatten();
    if let (Some(vi), Some(list)) = (vi, with(|s| s.list)) {
        unsafe {
            let _ = SetFocus(Some(list));
            SendMessageW(list, LVM_EDITLABELW, Some(WPARAM(vi)), None);
        }
    }
}

fn rename_selected() {
    let Some((sel, list, can)) = with(|s| (s.selected(), s.list, matches!(s.loc, Loc::Folder(_))))
    else {
        return;
    };
    if !can || sel.is_empty() {
        return;
    }
    if sel.len() > 1 {
        rename_many();
        return;
    }
    let vi = unsafe {
        SendMessageW(
            list,
            LVM_GETNEXTITEM,
            Some(WPARAM(usize::MAX)),
            Some(LPARAM(LVNI_FOCUSED as isize)),
        )
        .0
    };
    let vi = if vi >= 0 {
        vi
    } else {
        unsafe {
            SendMessageW(
                list,
                LVM_GETNEXTITEM,
                Some(WPARAM(usize::MAX)),
                Some(LPARAM(LVNI_SELECTED as isize)),
            )
            .0
        }
    };
    if vi >= 0 {
        unsafe {
            let _ = SetFocus(Some(list));
            SendMessageW(list, LVM_EDITLABELW, Some(WPARAM(vi as usize)), None);
        }
    }
}

/// 一括リネームの画面（選んだ項目。なければ表示しているすべての項目）。
fn rename_many() {
    let Some((items, hwnd, dirs)) = with(|s| {
        if !matches!(s.loc, Loc::Folder(_)) {
            return None;
        }
        let mut sel = s.selected();
        if sel.is_empty() {
            sel = s.view.clone();
        }
        let items: Vec<yy_files::rename::Item> = sel
            .iter()
            .map(|&i| {
                let r = &s.rows[i];
                yy_files::rename::Item {
                    path: r.path.clone(),
                    dir: r.dir,
                    size: r.size,
                    mtime: r.mtime,
                    ctime: r.ctime,
                }
            })
            .collect();
        Some((items, s.hwnd, s.dirs.clone()))
    })
    .flatten() else {
        return;
    };
    if items.is_empty() {
        return;
    }
    super::renamedlg::open(hwnd, items, dirs, WM_APP_RELOAD);
}

fn copy_paths() {
    let Some(text) = with(|s| {
        s.selected()
            .iter()
            .map(|&i| {
                let r = &s.rows[i];
                match s.entry(r) {
                    Some(e) => format!("{}\\{}", r.path.display(), e.path.replace('/', "\\")),
                    None => r.path.to_string_lossy().into_owned(),
                }
            })
            .collect::<Vec<_>>()
            .join("\r\n")
    }) else {
        return;
    };
    if !text.is_empty() {
        let _ = crate::clipboard::set_text(owner(), &text, false);
    }
}

fn select_all(invert: bool) {
    with(|s| unsafe {
        if invert {
            for vi in 0..s.view.len() {
                let st = SendMessageW(
                    s.list,
                    LVM_GETITEMSTATE,
                    Some(WPARAM(vi)),
                    Some(LPARAM(LVIS_SELECTED.0 as isize)),
                )
                .0 as u32;
                let it = LVITEMW {
                    stateMask: LVIS_SELECTED,
                    state: LIST_VIEW_ITEM_STATE_FLAGS(st ^ LVIS_SELECTED.0),
                    ..Default::default()
                };
                SendMessageW(
                    s.list,
                    LVM_SETITEMSTATE,
                    Some(WPARAM(vi)),
                    Some(LPARAM(&it as *const _ as isize)),
                );
            }
        } else {
            let it = LVITEMW {
                stateMask: LVIS_SELECTED,
                state: LVIS_SELECTED,
                ..Default::default()
            };
            SendMessageW(
                s.list,
                LVM_SETITEMSTATE,
                Some(WPARAM(usize::MAX)),
                Some(LPARAM(&it as *const _ as isize)),
            );
        }
        s.update_status();
    });
}

/// 選んだ圧縮ファイルの中の項目を一時フォルダに展開する（ドラッグ・コピー用）。
fn extract_selection_to_temp() -> Option<Vec<PathBuf>> {
    let (temp, tops, total) = with(|s| {
        let (_, a) = s.archive.clone()?;
        let n = s.temp_root.join(format!("out-{}", s.generation));
        let mut tops = Vec::new();
        let mut total = 0;
        for r in s.selected() {
            if let Some(i) = s.rows[r].entry {
                tops.push(a.entries[i].name().to_owned());
                total += if a.entries[i].dir {
                    a.under(&a.entries[i].path)
                        .iter()
                        .map(|&k| a.entries[k].size)
                        .sum()
                } else {
                    a.entries[i].size
                };
            }
        }
        Some((n, tops, total))
    })
    .flatten()?;
    if tops.is_empty() {
        return None;
    }
    if total > DRAG_EXTRACT_MAX {
        error_box(
            owner(),
            &format!(
                "大きすぎます（{}）。「展開してコピー...」で展開先を選んでください。",
                crate::util::human_size(total)
            ),
        );
        return None;
    }
    let _ = std::fs::remove_dir_all(&temp);
    unsafe {
        SetCursor(LoadCursorW(None, IDC_WAIT).ok());
    }
    match extract_selected(&temp) {
        Ok(_) => Some(tops.into_iter().map(|n| temp.join(n)).collect()),
        Err(e) => {
            error_box(owner(), &format!("展開できませんでした。\n{e}"));
            None
        }
    }
}

fn begin_drag() {
    let Some((paths, in_archive)) = with(|s| (s.selected_paths(), s.in_archive())) else {
        return;
    };
    let paths = if in_archive {
        match extract_selection_to_temp() {
            Some(p) => p,
            None => return,
        }
    } else {
        paths
    };
    if paths.is_empty() {
        return;
    }
    let effect = shell::drag_out(owner(), &paths);
    if !in_archive && effect.is_some() {
        reload_soon();
    }
}

/// ファイルが落とされた（Shift で移動・Ctrl でコピー。何もなければ同じドライブは移動、違えばコピー）。
fn on_drop(drop: HDROP) {
    let paths = shell::hdrop_paths(drop);
    let mut pt = POINT::default();
    unsafe {
        let _ = windows::Win32::UI::Shell::DragQueryPoint(drop, &mut pt);
        DragFinish(drop);
    }
    // 一覧のフォルダの上なら、そのフォルダへ
    let target = with(|s| {
        let mut p = pt;
        unsafe {
            let _ = windows::Win32::Graphics::Gdi::ClientToScreen(s.hwnd, &mut p);
            let _ = ScreenToClient(s.list, &mut p);
            let mut hit = LVHITTESTINFO {
                pt: p,
                ..Default::default()
            };
            let vi = SendMessageW(
                s.list,
                LVM_HITTEST,
                None,
                Some(LPARAM(&mut hit as *mut _ as isize)),
            )
            .0;
            if vi >= 0
                && let Some(&ri) = s.view.get(vi as usize)
                && s.rows[ri].dir
                && s.rows[ri].entry.is_none()
            {
                return Some(s.rows[ri].path.clone());
            }
        }
        match &s.loc {
            Loc::Folder(p) => Some(p.clone()),
            _ => None,
        }
    })
    .flatten();
    let Some(dest) = target else {
        error_box(owner(), "ここには置けません（フォルダを開いてください）。");
        return;
    };
    let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
    let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
    let root = |p: &Path| {
        p.components()
            .next()
            .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
    };
    let same = paths.first().is_some_and(|p| root(p) == root(&dest));
    let op = if shift {
        Op::Move
    } else if ctrl || !same {
        Op::Copy
    } else {
        Op::Move
    };
    // 自分の中・同じ場所へは何もしない
    let paths: Vec<PathBuf> = paths
        .into_iter()
        .filter(|p| p.parent() != Some(dest.as_path()) && !dest.starts_with(p))
        .collect();
    do_file_op(op, paths, Some(dest));
}

// ---- 右クリック ----

fn context_menu(screen: POINT) {
    let Some((sel_rows, paths, in_archive, loc, list, pinned)) = with(|s| {
        let sel = s.selected();
        let rows: Vec<Row> = sel.iter().map(|&i| s.rows[i].clone()).collect();
        (
            rows,
            s.selected_paths(),
            s.in_archive(),
            s.loc.clone(),
            s.list,
            s.st.pinned.clone(),
        )
    }) else {
        return;
    };
    let at = if screen.x == -1 && screen.y == -1 {
        // キーボード（Shift+F10）: 選んだ項目の位置
        let mut rc = RECT {
            left: LVIR_LABEL as i32,
            ..Default::default()
        };
        unsafe {
            let vi = SendMessageW(
                list,
                LVM_GETNEXTITEM,
                Some(WPARAM(usize::MAX)),
                Some(LPARAM(LVNI_FOCUSED as isize)),
            )
            .0;
            if vi >= 0 {
                SendMessageW(
                    list,
                    LVM_GETITEMRECT,
                    Some(WPARAM(vi as usize)),
                    Some(LPARAM(&mut rc as *mut _ as isize)),
                );
            }
            let mut p = POINT {
                x: rc.left + 8,
                y: rc.bottom,
            };
            let _ = windows::Win32::Graphics::Gdi::ClientToScreen(list, &mut p);
            p
        }
    } else {
        screen
    };
    let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
    let o = owner();
    let folder = match &loc {
        Loc::Folder(p) => Some(p.clone()),
        _ => None,
    };
    unsafe {
        let Ok(m) = CreatePopupMenu() else { return };
        let add = |id: u32, t: &str, on: bool| {
            let flags = if on { MF_STRING } else { MF_STRING | MF_GRAYED };
            let _ = AppendMenuW(m, flags, id as usize, &HSTRING::from(t));
        };
        let sep = || {
            let _ = AppendMenuW(m, MF_SEPARATOR, 0, None);
        };
        let one = sel_rows.len() == 1;
        if sel_rows.is_empty() {
            add(
                CM_NEW_FOLDER,
                "新しいフォルダ(&W)\tCtrl+Shift+N",
                folder.is_some(),
            );
            add(
                CM_PASTE,
                "貼り付け(&P)\tCtrl+V",
                folder.is_some() && shell::has_clipboard_files(),
            );
            sep();
            add(
                CM_FLAT,
                "フラットビュー(&F)\tCtrl+Shift+F",
                !matches!(loc, Loc::Computer),
            );
            add(
                CM_RENAME_MANY,
                "すべてを一括リネーム(&M)...",
                folder.is_some(),
            );
            add(CM_SELECT_ALL, "すべて選択(&A)\tCtrl+A", true);
            add(CM_REFRESH, "最新の情報に更新(&E)\tF5", true);
            sep();
            if let Some(f) = &folder {
                let pinned_here = pinned.iter().any(|p| p == f);
                add(
                    if pinned_here { CM_UNPIN } else { CM_PIN },
                    if pinned_here {
                        "よく使うフォルダから外す"
                    } else {
                        "よく使うフォルダにピン留め"
                    },
                    true,
                );
                add(CM_TERMINAL, "ターミナル（yyterm）で開く(&T)", true);
                add(CM_EXPLORER, "エクスプローラーで開く(&X)", true);
                add(CM_PROPS, "プロパティ(&R)\tAlt+Enter", true);
            }
        } else {
            add(CM_OPEN, "開く(&O)\tEnter", true);
            if !in_archive {
                add(
                    CM_OPEN_WITH,
                    "プログラムから開く(&H)...",
                    one && !sel_rows[0].dir,
                );
                add(
                    CM_EDITOR,
                    "yyeditor で開く(&Y)",
                    sel_rows.iter().all(|r| !r.dir),
                );
                if one && sel_rows[0].dir {
                    add(CM_TERMINAL, "ターミナル（yyterm）で開く(&T)", true);
                    let p = &sel_rows[0].path;
                    let pinned_here = pinned.iter().any(|q| q == p);
                    add(
                        if pinned_here { CM_UNPIN } else { CM_PIN },
                        if pinned_here {
                            "よく使うフォルダから外す"
                        } else {
                            "よく使うフォルダにピン留め"
                        },
                        true,
                    );
                }
            }
            add(
                CM_LOCATION,
                "ファイルのある場所を開く(&L)",
                one && !in_archive,
            );
            sep();
            if in_archive {
                add(CM_EXTRACT, "展開してコピー(&E)...", true);
                add(CM_COPY, "コピー(&C)\tCtrl+C", true);
            } else {
                add(CM_CUT, "切り取り(&T)\tCtrl+X", true);
                add(CM_COPY, "コピー(&C)\tCtrl+C", true);
                add(
                    CM_PASTE,
                    "フォルダに貼り付け(&P)",
                    one && sel_rows[0].dir && shell::has_clipboard_files(),
                );
                add(CM_COPY_TO, "フォルダへコピー(&Y)...", true);
                add(CM_MOVE_TO, "フォルダへ移動(&V)...", true);
                sep();
                add(CM_RENAME, "名前の変更(&M)\tF2", one);
                add(CM_RENAME_MANY, "一括リネーム(&B)...\tCtrl+Shift+R", true);
                add(CM_DELETE, "削除(&D)\tDel", true);
            }
            add(CM_COPY_PATH, "パスのコピー(&A)", true);
            if !in_archive {
                add(CM_PROPS, "プロパティ(&R)\tAlt+Enter", true);
            }
        }
        // シェルのメニュー（同じフォルダの項目だけ）
        let shell_paths: Vec<PathBuf> = if sel_rows.is_empty() {
            Vec::new()
        } else {
            paths.clone()
        };
        if !in_archive && !shell_paths.is_empty() {
            let sub = CreatePopupMenu().ok();
            if let Some(sub) = sub {
                if shell::add_shell_menu(o, sub, &shell_paths, shift) {
                    sep();
                    let _ = AppendMenuW(m, MF_POPUP, sub.0 as usize, w!("Windows のメニュー(&W)"));
                } else {
                    let _ = DestroyMenu(sub);
                }
            }
        }
        let cmd = TrackPopupMenu(
            m,
            TPM_RETURNCMD | TPM_RIGHTBUTTON,
            at.x,
            at.y,
            None,
            o,
            None,
        )
        .0 as u32;
        let _ = DestroyMenu(m);
        if cmd >= shell::SHELL_FIRST {
            match shell::shell_verb(cmd).as_deref() {
                Some("rename") => rename_selected(),
                _ => {
                    shell::invoke_shell(o, cmd, at);
                    reload_soon();
                }
            }
            shell::end_shell_menu();
            return;
        }
        shell::end_shell_menu();
        let target_dir = sel_rows
            .first()
            .filter(|r| r.dir && r.entry.is_none())
            .map(|r| r.path.clone());
        match cmd {
            CM_OPEN => {
                let rows = with(|s| s.selected()).unwrap_or_default();
                open_rows(&rows);
            }
            CM_OPEN_WITH => {
                if let Some(p) = paths.first() {
                    shell::open_with(o, p);
                }
            }
            CM_EDITOR => {
                for p in &paths {
                    super::open_in_editor(o, p, 0);
                }
            }
            CM_TERMINAL => {
                if let Some(d) = target_dir.clone().or(folder.clone()) {
                    open_terminal(o, &d);
                }
            }
            CM_EXPLORER => {
                if let Some(f) = &folder {
                    let _ = crate::app::workspacemode::open_explorer(f);
                }
            }
            CM_LOCATION => {
                if let Some(p) = paths.first() {
                    open_path(p);
                }
            }
            CM_COPY_PATH => copy_paths(),
            CM_CUT => copy_selected(true),
            CM_COPY => copy_selected(false),
            CM_PASTE => paste(target_dir),
            CM_COPY_TO | CM_MOVE_TO => {
                if let Some(d) = crate::grepdlg::browse_folder(o) {
                    do_file_op(
                        if cmd == CM_COPY_TO {
                            Op::Copy
                        } else {
                            Op::Move
                        },
                        paths,
                        Some(d),
                    );
                }
            }
            CM_EXTRACT => {
                if let Some(d) = crate::grepdlg::browse_folder(o) {
                    match extract_selected(&d) {
                        Ok(n) => with(|s| {
                            s.set_status(&format!(
                                "{} に展開しました（{}）",
                                d.display(),
                                crate::util::human_size(n)
                            ))
                        })
                        .unwrap_or(()),
                        Err(e) => error_box(o, &format!("展開できませんでした。\n{e}")),
                    }
                }
            }
            CM_RENAME => rename_selected(),
            CM_RENAME_MANY => rename_many(),
            CM_DELETE => delete_selected(GetKeyState(VK_SHIFT.0 as i32) < 0),
            CM_NEW_FOLDER => new_folder(),
            CM_PIN | CM_UNPIN => {
                let p = target_dir.or(folder);
                with(|s| {
                    if let Some(p) = p {
                        s.st.pinned.retain(|q| *q != p);
                        if cmd == CM_PIN {
                            s.st.pinned.push(p);
                        }
                        s.save_state();
                        s.fill_tree();
                    }
                });
            }
            CM_PROPS => {
                let ps = if paths.is_empty() {
                    folder.into_iter().collect()
                } else {
                    paths
                };
                shell::properties(o, &ps);
            }
            CM_SELECT_ALL => select_all(false),
            CM_INVERT => select_all(true),
            CM_REFRESH => reload_keep_selection(),
            CM_FLAT => toggle_flat(),
            _ => {}
        }
    }
}

fn open_terminal(owner: HWND, dir: &Path) {
    let exe = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join("yyterm.exe")))
        .filter(|e| e.is_file());
    let Some(exe) = exe else {
        error_box(owner, "yyterm.exe が同じフォルダにありません。");
        return;
    };
    if let Err(e) = std::process::Command::new(exe).arg(dir).spawn() {
        error_box(owner, &format!("yyterm を起動できません: {e}"));
    }
}

fn view_menu() {
    let Some((st, btn)) = with(|s| {
        (
            s.st.clone(),
            s.buttons
                .iter()
                .find(|(i, _)| *i == ID_VIEW)
                .map(|(_, h)| *h),
        )
    }) else {
        return;
    };
    unsafe {
        let Ok(m) = CreatePopupMenu() else { return };
        let add = |id: u32, t: &str, checked: bool| {
            let flags = if checked {
                MF_STRING | MF_CHECKED
            } else {
                MF_STRING
            };
            let _ = AppendMenuW(m, flags, id as usize, &HSTRING::from(t));
        };
        add(VM_DETAILS, "詳細(&D)", st.view == "details");
        add(VM_LIST, "一覧(&L)", st.view == "list");
        add(VM_ICONS, "大きいアイコン(&I)", st.view == "icons");
        let _ = AppendMenuW(m, MF_SEPARATOR, 0, None);
        add(VM_HIDDEN, "隠しファイルを表示(&H)", st.show_hidden);
        add(VM_EXT, "拡張子を表示(&E)", st.show_extensions);
        add(
            VM_FLAT_FOLDERS,
            "フラットビューにフォルダも出す(&F)",
            st.flat_folders,
        );
        let mut rc = RECT::default();
        if let Some(b) = btn {
            let _ = GetWindowRect(b, &mut rc);
        }
        let cmd =
            TrackPopupMenu(m, TPM_RETURNCMD, rc.left, rc.bottom, None, owner(), None).0 as u32;
        let _ = DestroyMenu(m);
        with(|s| {
            match cmd {
                VM_DETAILS => s.st.view = "details".into(),
                VM_LIST => s.st.view = "list".into(),
                VM_ICONS => s.st.view = "icons".into(),
                VM_HIDDEN => s.st.show_hidden = !s.st.show_hidden,
                VM_EXT => s.st.show_extensions = !s.st.show_extensions,
                VM_FLAT_FOLDERS => s.st.flat_folders = !s.st.flat_folders,
                _ => return,
            }
            s.apply_view();
            s.save_state();
            s.rebuild_view();
            if cmd == VM_HIDDEN {
                s.fill_tree();
            }
        });
    }
}

fn apply_filter() {
    with(|s| {
        let t = text_of(s.filter);
        if t.trim().is_empty() {
            s.query = None;
            s.query_error = None;
        } else {
            match yy_files::search::parse_query(&t, super::now_ns(), super::tz_offset()) {
                Ok(q) => {
                    s.query = Some(q);
                    s.query_error = None;
                }
                Err(e) => {
                    s.query_error = Some(e);
                    return s.update_status();
                }
            }
        }
        s.rebuild_view();
    });
}

/// キーの処理（fm のメッセージ ループから。処理したら `true`）。
pub(super) fn key(msg: &MSG) -> bool {
    let Some((hwnd, list, address, filter, tree)) =
        with(|s| (s.hwnd, s.list, s.address, s.filter, s.tree))
    else {
        return false;
    };
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() {
            return false;
        }
    }
    let vk = VIRTUAL_KEY(msg.wParam.0 as u16);
    let down = |k: VIRTUAL_KEY| unsafe { GetKeyState(k.0 as i32) } < 0;
    let (ctrl, shift, alt) = (
        down(VK_CONTROL),
        down(VK_SHIFT),
        msg.message == WM_SYSKEYDOWN || down(VK_MENU),
    );
    let on_list = msg.hwnd == list;
    // 名前の変更の欄の中は触らない
    let editing = unsafe { SendMessageW(list, LVM_GETEDITCONTROL, None, None).0 } != 0;
    if editing {
        return false;
    }
    let in_edit = msg.hwnd == address || msg.hwnd == filter;
    match vk {
        VK_LEFT if alt => back(),
        VK_RIGHT if alt => forward(),
        VK_UP if alt => up(),
        VK_BROWSER_BACK => back(),
        VK_BROWSER_FORWARD => forward(),
        VK_BACK if on_list || msg.hwnd == tree => up(),
        VK_F5 => reload_keep_selection(),
        VK_RETURN if msg.hwnd == address => go_address(),
        VK_RETURN if msg.hwnd == filter => unsafe {
            let _ = SetFocus(Some(list));
        },
        VK_ESCAPE if msg.hwnd == filter => {
            set_text(filter, "");
            apply_filter();
        }
        VK_ESCAPE if with(|s| s.loading) == Some(true) => {
            with(|s| {
                if let Some(c) = s.cancel.take() {
                    c.store(true, Ordering::Relaxed);
                }
                s.loading = false;
                s.rebuild_view();
            });
        }
        VK_RETURN if on_list && alt => {
            let ps = with(|s| s.selected_paths()).unwrap_or_default();
            shell::properties(hwnd, &ps);
        }
        VK_RETURN if on_list => {
            let rows = with(|s| s.selected()).unwrap_or_default();
            open_rows(&rows);
        }
        VK_D if alt => unsafe {
            let _ = SetFocus(Some(address));
            SendMessageW(address, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
        },
        VK_L if ctrl => unsafe {
            let _ = SetFocus(Some(address));
            SendMessageW(address, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
        },
        VK_F4 if !alt => unsafe {
            let _ = SetFocus(Some(address));
        },
        VK_F if ctrl && shift => toggle_flat(),
        VK_F if ctrl => focus_filter(),
        VK_F3 => focus_filter(),
        VK_R if ctrl && shift => rename_many(),
        VK_N if ctrl && shift => new_folder(),
        VK_F2 if on_list => rename_selected(),
        VK_DELETE if on_list => delete_selected(shift),
        VK_A if ctrl && on_list => select_all(false),
        VK_C if ctrl && on_list => copy_selected(false),
        VK_X if ctrl && on_list => copy_selected(true),
        VK_V if ctrl && !in_edit => paste(None),
        _ => return false,
    }
    true
}

fn on_list_notify(hdr: &NMHDR, lparam: LPARAM) -> LRESULT {
    match hdr.code {
        LVN_GETDISPINFOW => {
            let nm = unsafe { &mut *(lparam.0 as *mut NMLVDISPINFOW) };
            let row = nm.item.iItem.max(0) as usize;
            if nm.item.mask.0 & LVIF_TEXT.0 != 0
                && nm.item.cchTextMax > 0
                && !nm.item.pszText.is_null()
            {
                let text =
                    with(|s| s.cell(row, nm.item.iSubItem.max(0) as usize)).unwrap_or_default();
                let buf = unsafe {
                    std::slice::from_raw_parts_mut(nm.item.pszText.0, nm.item.cchTextMax as usize)
                };
                let mut n = 0;
                for u in text.encode_utf16().take(buf.len() - 1) {
                    buf[n] = u;
                    n += 1;
                }
                buf[n] = 0;
            }
            if nm.item.mask.0 & LVIF_IMAGE.0 != 0 {
                nm.item.iImage = with(|s| {
                    let r = s.view.get(row).map(|&i| s.rows[i].clone());
                    r.map(|r| s.icon(&r)).unwrap_or(0)
                })
                .unwrap_or(0);
            }
            LRESULT(0)
        }
        LVN_ODFINDITEMW => {
            // 頭の文字で探す
            let nm = unsafe { &*(lparam.0 as *const NMLVFINDITEMW) };
            if nm.lvfi.flags.0 & (LVFI_STRING.0 | LVFI_PARTIAL.0) == 0 || nm.lvfi.psz.is_null() {
                return LRESULT(-1);
            }
            let want = unsafe { nm.lvfi.psz.to_string() }
                .unwrap_or_default()
                .to_lowercase();
            let start = nm.iStart.max(0) as usize;
            let found = with(|s| {
                let n = s.view.len();
                (0..n).map(|k| (start + k) % n.max(1)).find(|&vi| {
                    s.display_name(&s.rows[s.view[vi]])
                        .to_lowercase()
                        .starts_with(&want)
                })
            })
            .flatten();
            LRESULT(found.map_or(-1, |v| v as isize))
        }
        LVN_COLUMNCLICK => {
            let nm = unsafe { &*(lparam.0 as *const NMLISTVIEW) };
            with(|s| {
                if let Some(&c) = s.cols.get(nm.iSubItem.max(0) as usize) {
                    s.sort = if s.sort.0 == c {
                        (c, !s.sort.1)
                    } else {
                        (c, true)
                    };
                    s.show_sort_arrow();
                    s.rebuild_view();
                }
            });
            LRESULT(0)
        }
        LVN_ITEMACTIVATE => {
            let rows = with(|s| s.selected()).unwrap_or_default();
            open_rows(&rows);
            LRESULT(0)
        }
        LVN_ITEMCHANGED | LVN_ODSTATECHANGED => {
            with(|s| s.update_status());
            LRESULT(0)
        }
        LVN_BEGINDRAG | LVN_BEGINRDRAG => {
            begin_drag();
            LRESULT(0)
        }
        LVN_BEGINLABELEDITW => {
            let nm = unsafe { &*(lparam.0 as *const NMLVDISPINFOW) };
            let info = with(|s| {
                let ri = *s.view.get(nm.item.iItem.max(0) as usize)?;
                let r = &s.rows[ri];
                (matches!(s.loc, Loc::Folder(_)) && r.entry.is_none())
                    .then(|| (r.name.clone(), r.dir))
            })
            .flatten();
            let Some((name, dir)) = info else {
                return LRESULT(1);
            };
            unsafe {
                let edit =
                    HWND(SendMessageW(hdr.hwndFrom, LVM_GETEDITCONTROL, None, None).0 as *mut _);
                // 表示は拡張子を隠していても、元の名前で直す
                set_text(edit, &name);
                let stem = if dir {
                    name.as_str()
                } else {
                    yy_files::rename::split_ext(&name, false).0
                };
                let n = stem.encode_utf16().count();
                SendMessageW(edit, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(n as isize)));
            }
            LRESULT(0)
        }
        LVN_ENDLABELEDITW => {
            let nm = unsafe { &*(lparam.0 as *const NMLVDISPINFOW) };
            if nm.item.pszText.is_null() {
                return LRESULT(0);
            }
            let new_name = unsafe { nm.item.pszText.to_string() }.unwrap_or_default();
            let new_name = new_name.trim().to_owned();
            let Some((path, old)) = with(|s| {
                let ri = *s.view.get(nm.item.iItem.max(0) as usize)?;
                Some((s.rows[ri].path.clone(), s.rows[ri].name.clone()))
            })
            .flatten() else {
                return LRESULT(0);
            };
            if new_name == old || new_name.is_empty() {
                return LRESULT(0);
            }
            let o = owner();
            if let Some(why) = yy_files::rename::invalid_reason(&new_name) {
                error_box(o, &format!("「{new_name}」は使えない名前です（{why}）。"));
                return LRESULT(0);
            }
            match shell::rename(o, &path, &new_name) {
                Ok(()) => {
                    let np = path.with_file_name(&new_name);
                    with(|s| {
                        if let Some(r) = s.rows.iter_mut().find(|r| r.path == path) {
                            r.name = new_name.clone();
                            r.path = np.clone();
                        }
                        s.rebuild_view();
                        s.select_paths(&[np], true);
                    });
                }
                Err(e) => error_box(o, &format!("名前を変えられませんでした。\n{e}")),
            }
            LRESULT(0)
        }
        _ => LRESULT(0),
    }
}

fn on_tree_notify(hdr: &NMHDR, lparam: LPARAM) -> LRESULT {
    match hdr.code {
        TVN_ITEMEXPANDINGW => {
            let nm = unsafe { &*(lparam.0 as *const NMTREEVIEWW) };
            if nm.action == TVE_EXPAND {
                with(|s| {
                    if let Some(i) = s.node_of(nm.itemNew.hItem) {
                        s.load_children(i);
                    }
                });
            }
            LRESULT(0)
        }
        TVN_SELCHANGEDW => {
            let nm = unsafe { &*(lparam.0 as *const NMTREEVIEWW) };
            // マウス・キーで選んだときだけ移る
            if nm.action == TVC_BYMOUSE || nm.action == TVC_BYKEYBOARD {
                let loc = with(|s| {
                    s.node_of(nm.itemNew.hItem)
                        .and_then(|i| s.nodes[i].loc.clone())
                })
                .flatten();
                if let Some(l) = loc {
                    go(l, true);
                }
            }
            LRESULT(0)
        }
        NM_RCLICK => {
            // ピン留めを外す
            let mut screen = POINT::default();
            unsafe {
                let _ = GetCursorPos(&mut screen);
            }
            let Some(tree) = with(|s| s.tree) else {
                return LRESULT(0);
            };
            let mut pt = screen;
            unsafe {
                let _ = ScreenToClient(tree, &mut pt);
            }
            let mut hit = TVHITTESTINFO {
                pt,
                ..Default::default()
            };
            let item = unsafe {
                HTREEITEM(
                    SendMessageW(
                        tree,
                        TVM_HITTEST,
                        None,
                        Some(LPARAM(&mut hit as *mut _ as isize)),
                    )
                    .0,
                )
            };
            let Some(Some(Loc::Folder(p))) =
                with(|s| s.node_of(item).and_then(|i| s.nodes[i].loc.clone()))
            else {
                return LRESULT(0);
            };
            let pinned = with(|s| s.st.pinned.contains(&p)).unwrap_or(false);
            unsafe {
                let Ok(m) = CreatePopupMenu() else {
                    return LRESULT(0);
                };
                let _ = AppendMenuW(m, MF_STRING, CM_OPEN as usize, w!("開く(&O)"));
                let _ = AppendMenuW(
                    m,
                    MF_STRING,
                    (if pinned { CM_UNPIN } else { CM_PIN }) as usize,
                    if pinned {
                        w!("よく使うフォルダから外す(&U)")
                    } else {
                        w!("よく使うフォルダにピン留め(&P)")
                    },
                );
                let _ = AppendMenuW(
                    m,
                    MF_STRING,
                    CM_TERMINAL as usize,
                    w!("ターミナル（yyterm）で開く(&T)"),
                );
                let _ = AppendMenuW(m, MF_STRING, CM_PROPS as usize, w!("プロパティ(&R)"));
                let cmd = TrackPopupMenu(
                    m,
                    TPM_RETURNCMD | TPM_RIGHTBUTTON,
                    screen.x,
                    screen.y,
                    None,
                    owner(),
                    None,
                )
                .0 as u32;
                let _ = DestroyMenu(m);
                match cmd {
                    CM_OPEN => go(Loc::Folder(p), true),
                    CM_PIN | CM_UNPIN => {
                        with(|s| {
                            s.st.pinned.retain(|q| *q != p);
                            if cmd == CM_PIN {
                                s.st.pinned.push(p.clone());
                            }
                            s.save_state();
                            s.fill_tree();
                        });
                    }
                    CM_TERMINAL => open_terminal(owner(), &p),
                    CM_PROPS => shell::properties(owner(), &[p]),
                    _ => {}
                }
            }
            LRESULT(1)
        }
        _ => LRESULT(0),
    }
}

extern "system" fn proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if let Some(r) = shell::menu_message(msg, wparam, lparam) {
        return r;
    }
    match msg {
        WM_SIZE => {
            with(|s| s.layout());
            LRESULT(0)
        }
        WM_COMMAND => {
            let id = loword(wparam.0) as u16;
            let code = hiword(wparam.0);
            match id {
                ID_BACK => back(),
                ID_FORWARD => forward(),
                ID_UP => up(),
                ID_REFRESH => reload_keep_selection(),
                ID_GO => go_address(),
                ID_FLAT if code == BN_CLICKED => toggle_flat(),
                ID_NEW_FOLDER => new_folder(),
                ID_RENAME_MANY => rename_many(),
                ID_VIEW => view_menu(),
                ID_FILTER if code == EN_CHANGE => unsafe {
                    SetTimer(Some(hwnd), TIMER_FILTER, 250, None);
                },
                _ => {}
            }
            LRESULT(0)
        }
        WM_TIMER => {
            unsafe {
                let _ = KillTimer(Some(hwnd), wparam.0);
            }
            match wparam.0 {
                TIMER_FILTER => apply_filter(),
                TIMER_RELOAD => reload_keep_selection(),
                _ => {}
            }
            LRESULT(0)
        }
        WM_NOTIFY => {
            let hdr = unsafe { &*(lparam.0 as *const NMHDR) };
            match hdr.idFrom as u16 {
                ID_LIST => on_list_notify(hdr, lparam),
                ID_TREE => on_tree_notify(hdr, lparam),
                _ => LRESULT(0),
            }
        }
        WM_CONTEXTMENU => {
            let list = with(|s| s.list).unwrap_or_default();
            if HWND(wparam.0 as *mut _) == list {
                let pt = POINT {
                    x: (lparam.0 & 0xffff) as i16 as i32,
                    y: ((lparam.0 >> 16) & 0xffff) as i16 as i32,
                };
                // 何もないところなら選びを外す
                if pt.x != -1 || pt.y != -1 {
                    let mut p = pt;
                    unsafe {
                        let _ = ScreenToClient(list, &mut p);
                        let mut hit = LVHITTESTINFO {
                            pt: p,
                            ..Default::default()
                        };
                        let vi = SendMessageW(
                            list,
                            LVM_HITTEST,
                            None,
                            Some(LPARAM(&mut hit as *mut _ as isize)),
                        )
                        .0;
                        if vi < 0 {
                            let none = LVITEMW {
                                stateMask: LVIS_SELECTED,
                                ..Default::default()
                            };
                            SendMessageW(
                                list,
                                LVM_SETITEMSTATE,
                                Some(WPARAM(usize::MAX)),
                                Some(LPARAM(&none as *const _ as isize)),
                            );
                        }
                    }
                }
                context_menu(pt);
                return LRESULT(0);
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_DROPFILES => {
            on_drop(HDROP(wparam.0 as *mut _));
            LRESULT(0)
        }
        WM_SETCURSOR => {
            let mut pt = POINT::default();
            unsafe {
                let _ = GetCursorPos(&mut pt);
                let _ = ScreenToClient(hwnd, &mut pt);
            }
            let on = with(|s| {
                let r = s.splitter_rect();
                pt.x >= r.left && pt.x < r.right && pt.y >= r.top && pt.y < r.bottom
            })
            .unwrap_or(false);
            if on || with(|s| s.dragging_splitter) == Some(true) {
                unsafe {
                    SetCursor(LoadCursorW(None, IDC_SIZEWE).ok());
                }
                return LRESULT(1);
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_LBUTTONDOWN => {
            let x = (lparam.0 & 0xffff) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xffff) as i16 as i32;
            let on = with(|s| {
                let r = s.splitter_rect();
                let on = x >= r.left && x < r.right && y >= r.top && y < r.bottom;
                s.dragging_splitter = on;
                on
            })
            .unwrap_or(false);
            if on {
                unsafe {
                    SetCapture(hwnd);
                }
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let x = (lparam.0 & 0xffff) as i16 as i32;
            with(|s| {
                if s.dragging_splitter {
                    s.st.tree_width = (x * 96 / s.dpi as i32).clamp(80, 1200);
                    s.layout();
                }
            });
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let was = with(|s| std::mem::replace(&mut s.dragging_splitter, false)).unwrap_or(false);
            if was {
                unsafe {
                    let _ = ReleaseCapture();
                }
                with(|s| s.save_state());
            }
            LRESULT(0)
        }
        WM_APP_ROWS => {
            on_rows(*unsafe { take::<Batch>(lparam) });
            LRESULT(0)
        }
        WM_APP_TREE => {
            let (generation, dirs) = *unsafe { take::<(u64, Vec<(String, PathBuf)>)>(lparam) };
            with(|s| s.on_tree_loaded(wparam.0, generation, dirs));
            LRESULT(0)
        }
        WM_APP_ARCHIVE => {
            let (generation, file, r) =
                *unsafe { take::<(u64, PathBuf, Result<Arc<Archive>, String>)>(lparam) };
            let err = with(|s| {
                if generation != s.generation {
                    return None;
                }
                s.loading = false;
                match r {
                    Ok(a) => {
                        s.archive = Some((file, a));
                        s.archive_rows();
                        s.rebuild_view();
                        None
                    }
                    Err(e) => {
                        s.update_status();
                        Some(e)
                    }
                }
            })
            .flatten();
            if let Some(e) = err {
                error_box(hwnd, &format!("圧縮ファイルの中を読めませんでした。\n{e}"));
                back();
            }
            LRESULT(0)
        }
        WM_APP_CHANGED => {
            if with(|s| s.generation as usize == wparam.0 && !s.loading) == Some(true) {
                unsafe {
                    SetTimer(Some(hwnd), TIMER_RELOAD, 400, None);
                }
            }
            LRESULT(0)
        }
        WM_APP_PROPS => {
            let batch = *unsafe { take::<Vec<(PathBuf, Props)>>(lparam) };
            with(|s| {
                for (p, props) in batch {
                    s.props_pending.remove(&p);
                    s.props.insert(p, props);
                }
                s.rebuild_view();
            });
            LRESULT(0)
        }
        WM_APP_EXTRACTED => {
            let r = *unsafe { take::<Result<PathBuf, String>>(lparam) };
            on_extracted(wparam.0 != 0, r);
            LRESULT(0)
        }
        WM_APP_RELOAD => {
            reload_keep_selection();
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_addresses_and_parents() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        std::fs::create_dir(d.join("sub")).unwrap();
        std::fs::write(d.join("a.zip"), b"PK").unwrap();
        std::fs::write(d.join("memo.txt"), b"x").unwrap();
        assert_eq!(Loc::parse("pc"), Some(Loc::Computer));
        assert_eq!(
            Loc::parse(&d.join("sub").to_string_lossy()),
            Some(Loc::Folder(d.join("sub")))
        );
        // ファイルならそのフォルダ、圧縮ファイルなら中
        assert_eq!(
            Loc::parse(&d.join("memo.txt").to_string_lossy()),
            Some(Loc::Folder(d.to_path_buf()))
        );
        assert_eq!(
            Loc::parse(&format!("\"{}\"", d.join("a.zip").display())),
            Some(Loc::Archive {
                file: d.join("a.zip"),
                inner: String::new()
            })
        );
        let inside = Loc::parse(&d.join("a.zip").join("図").join("2024").to_string_lossy());
        assert_eq!(
            inside,
            Some(Loc::Archive {
                file: d.join("a.zip"),
                inner: "図/2024".into()
            })
        );
        assert_eq!(
            Loc::parse(&d.join("nothing").join("x").to_string_lossy()),
            None
        );
        // 上へ
        let l = inside.unwrap();
        assert_eq!(
            l.address(),
            format!("{}\\図\\2024", d.join("a.zip").display())
        );
        let l = l.parent().unwrap();
        assert_eq!(
            l,
            Loc::Archive {
                file: d.join("a.zip"),
                inner: "図".into()
            }
        );
        let l = l.parent().unwrap().parent().unwrap();
        assert_eq!(l, Loc::Folder(d.to_path_buf()));
        assert_eq!(
            Loc::Folder(PathBuf::from(r"C:\")).parent(),
            Some(Loc::Computer)
        );
        assert_eq!(Loc::Computer.parent(), None);
    }

    #[test]
    fn expands_environment_variables() {
        let home = std::env::var("USERPROFILE").unwrap_or_default();
        assert_eq!(expand_env("%USERPROFILE%\\x"), format!("{home}\\x"));
        assert_eq!(expand_env("100%"), "100%");
        assert_eq!(expand_env("%NO_SUCH_VAR_YY%"), "%NO_SUCH_VAR_YY%");
    }

    #[test]
    fn filetime_to_nanos() {
        // 1970-01-01
        assert_eq!(ft_ns(116_444_736_000_000_000), 0);
        assert_eq!(ft_ns(116_444_736_000_000_001), 100);
        assert_eq!(ft_ns(0), 0);
    }
}
