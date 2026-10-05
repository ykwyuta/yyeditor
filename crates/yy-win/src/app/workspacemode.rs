//! ワークスペース（VS Code のワークスペースのように、複数の起点のフォルダをまとめて扱う）。
//!
//! 左側のサイドバー（ツリー ビュー）に起点のフォルダを並べ、フォルダを開いたときに中身を読む。
//! ファイルはダブルクリック・Enter で開き、右クリックのメニューからフォルダをターミナルや
//! エクスプローラーで開ける。ワークスペースは `*.yyworkspace` に保存する
//! （[`yy_config::workspace`]）。

use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use windows::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL, FILE_FLAGS_AND_ATTRIBUTES,
};
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::Shell::{
    SHFILEINFOW, SHGFI_OPENICON, SHGFI_SMALLICON, SHGFI_SYSICONINDEX, SHGFI_USEFILEATTRIBUTES,
    SHGetFileInfoW,
};
use yy_config::workspace::{self, Workspace};

use super::*;

pub(crate) const ID_WS_SIDEBAR: u16 = 1300;
pub(crate) const ID_WS_ADD_FOLDER: u16 = 1301;
pub(crate) const ID_WS_OPEN: u16 = 1302;
pub(crate) const ID_WS_SAVE_AS: u16 = 1303;
pub(crate) const ID_WS_NEW: u16 = 1304;
pub(crate) const ID_WS_REFRESH: u16 = 1305;
/// サイドバーのツリー ビューの ID
const ID_TREE: u16 = 1310;

// 右クリックのメニュー
const CM_OPEN: u32 = 1;
const CM_TERMINAL: u32 = 2;
const CM_EXPLORER: u32 = 3;
const CM_COPY_PATH: u32 = 4;
const CM_COPY_RELATIVE: u32 = 5;
const CM_GREP: u32 = 6;
const CM_REFRESH: u32 = 7;
const CM_REMOVE_ROOT: u32 = 8;
const CM_ADD_FOLDER: u32 = 9;

/// サイドバーとツリーの境界の幅（96 DPI でのピクセル）
const SPLITTER: i32 = 5;

/// ツリーの 1 項目。
struct Node {
    path: PathBuf,
    is_dir: bool,
    /// 中身を読んだ（フォルダ）
    loaded: bool,
    /// 起点のフォルダ
    root: bool,
}

/// ワークスペースとサイドバーの状態。
pub(crate) struct WorkspacePane {
    pub tree: HWND,
    pub visible: bool,
    /// サイドバーの幅（96 DPI でのピクセル）
    pub width: i32,
    pub dragging: bool,
    /// 境界の範囲（フレームのクライアント座標）
    pub splitter: RECT,
    pub workspace: Workspace,
    /// 保存先（名前を付けていなければ設定フォルダの untitled.yyworkspace）
    pub file: Option<PathBuf>,
    /// 項目（ツリーの項目の lParam が番号）
    nodes: Vec<Node>,
    /// 拡張子ごとのアイコンの番号（システムのイメージ リスト）
    icons: std::collections::HashMap<String, (i32, i32)>,
}

impl WorkspacePane {
    /// 名前を付けて保存したワークスペースか。
    fn named(&self) -> bool {
        self.file
            .as_deref()
            .is_some_and(|f| workspace::config_file(workspace::UNTITLED_FILE).as_deref() != Some(f))
    }
}

/// サイドバーを作る（最後に使ったワークスペースを開き直す）。
pub(crate) fn create_pane(frame: HWND, instance: HINSTANCE) -> Result<WorkspacePane> {
    let tree = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            WC_TREEVIEWW,
            None,
            WS_CHILD
                | WS_TABSTOP
                | WINDOW_STYLE(
                    TVS_HASBUTTONS
                        | TVS_LINESATROOT
                        | TVS_SHOWSELALWAYS
                        | TVS_FULLROWSELECT
                        | TVS_INFOTIP,
                ),
            0,
            0,
            0,
            0,
            Some(frame),
            Some(HMENU(ID_TREE as isize as *mut _)),
            Some(instance),
            None,
        )
        .context("CreateWindowExW(tree)")?
    };
    unsafe {
        let _ = windows::Win32::UI::Controls::SetWindowTheme(tree, w!("Explorer"), None);
        SendMessageW(
            tree,
            TVM_SETEXTENDEDSTYLE,
            Some(WPARAM(TVS_EX_DOUBLEBUFFER as usize)),
            Some(LPARAM(TVS_EX_DOUBLEBUFFER as isize)),
        );
        // システムの小さいアイコン（エクスプローラーと同じ）
        let mut info = SHFILEINFOW::default();
        let list = SHGetFileInfoW(
            w!("folder"),
            FILE_ATTRIBUTE_DIRECTORY,
            Some(&mut info),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_SYSICONINDEX | SHGFI_SMALLICON | SHGFI_USEFILEATTRIBUTES,
        );
        if list != 0 {
            SendMessageW(
                tree,
                TVM_SETIMAGELIST,
                Some(WPARAM(TVSIL_NORMAL as usize)),
                Some(LPARAM(list as isize)),
            );
        }
    }
    let file = workspace::last_used().or_else(|| workspace::config_file(workspace::UNTITLED_FILE));
    let ws = file
        .as_deref()
        .and_then(|f| Workspace::load(f).ok())
        .unwrap_or_default();
    Ok(WorkspacePane {
        tree,
        visible: !ws.folders.is_empty(),
        width: 240,
        dragging: false,
        splitter: RECT::default(),
        workspace: ws,
        file,
        nodes: Vec::new(),
        icons: Default::default(),
    })
}

/// フォルダをターミナルで開く。`custom` は設定のコマンド（空なら Windows Terminal、
/// なければコマンド プロンプト）。
pub(crate) fn open_terminal(dir: &Path, custom: &[String]) -> std::result::Result<(), String> {
    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
    if let Some((cmd, args)) = custom.split_first() {
        let d = dir.to_string_lossy();
        return Command::new(cmd)
            .args(args.iter().map(|a| a.replace("{dir}", &d)))
            .current_dir(dir)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("{cmd}: {e}"));
    }
    if Command::new("wt.exe")
        .arg("-d")
        .arg(dir)
        .current_dir(dir)
        .spawn()
        .is_ok()
    {
        return Ok(());
    }
    Command::new("cmd.exe")
        .current_dir(dir)
        .creation_flags(CREATE_NEW_CONSOLE)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("cmd.exe: {e}"))
}

/// エクスプローラーで開く（ファイルならそのフォルダを開いて選ぶ）。
pub(crate) fn open_explorer(path: &Path) -> std::result::Result<(), String> {
    let mut cmd = Command::new("explorer.exe");
    if path.is_dir() {
        cmd.arg(path);
    } else {
        cmd.raw_arg(format!("/select,\"{}\"", path.display()));
    }
    cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// ツリーの項目の文字列。
fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

impl App {
    /// サイドバーの幅（ピクセル。表示していなければ 0）と境界の幅。
    fn sidebar_px(&self) -> (i32, i32) {
        if !self.ws.visible {
            return (0, 0);
        }
        let dpi = unsafe { GetDpiForWindow(self.frame) }.max(96) as i32;
        (self.ws.width * dpi / 96, SPLITTER * dpi / 96)
    }

    /// サイドバーを置き、その右の（エディタなどを置く）左端を返す。
    pub(crate) fn layout_sidebar(&mut self, width: i32, height: i32) -> i32 {
        let (sw, split) = self.sidebar_px();
        unsafe {
            if sw == 0 {
                let _ = ShowWindow(self.ws.tree, SW_HIDE);
                self.ws.splitter = RECT::default();
                return 0;
            }
            let sw = sw.min((width - split - 100).max(60));
            let _ = MoveWindow(self.ws.tree, 0, 0, sw, height, true);
            let _ = ShowWindow(self.ws.tree, SW_SHOWNA);
            self.ws.splitter = RECT {
                left: sw,
                top: 0,
                right: sw + split,
                bottom: height,
            };
            sw + split
        }
    }

    /// フレームのクライアント座標がサイドバーの境界の上か。
    pub(crate) fn on_sidebar_splitter(&self, x: i32, y: i32) -> bool {
        let r = self.ws.splitter;
        self.ws.visible && x >= r.left && x < r.right && y >= r.top && y < r.bottom
    }

    /// 境界をドラッグして `x` に動かす。
    pub(crate) fn drag_sidebar_splitter(&mut self, x: i32) {
        let dpi = unsafe { GetDpiForWindow(self.frame) }.max(96) as i32;
        self.ws.width = (x * 96 / dpi).clamp(80, 2000);
        self.layout_children();
    }

    /// サイドバーの表示を切り替える。
    pub(crate) fn toggle_sidebar(&mut self) {
        self.ws.visible = !self.ws.visible;
        if self.ws.visible && self.ws.nodes.is_empty() {
            self.rebuild_tree();
        }
        self.layout_children();
        self.update_workspace_menu();
        unsafe {
            let _ = SetFocus(Some(if self.ws.visible {
                self.ws.tree
            } else {
                self.view
            }));
        }
    }

    pub(crate) fn update_workspace_menu(&self) {
        unsafe {
            let menu = GetMenu(self.frame);
            let flag = if self.ws.visible {
                MF_CHECKED
            } else {
                MF_UNCHECKED
            };
            CheckMenuItem(menu, ID_WS_SIDEBAR as u32, (MF_BYCOMMAND | flag).0);
        }
    }

    /// 拡張子（フォルダは空）のアイコンの番号（通常, 開いたとき）。
    fn icon_for(&mut self, path: &Path, is_dir: bool) -> (i32, i32) {
        let key = if is_dir {
            String::new()
        } else {
            path.extension()
                .map(|e| e.to_string_lossy().to_lowercase())
                .unwrap_or_else(|| "-".into())
        };
        if let Some(&v) = self.ws.icons.get(&key) {
            return v;
        }
        let attrs = if is_dir {
            FILE_ATTRIBUTE_DIRECTORY
        } else {
            FILE_ATTRIBUTE_NORMAL
        };
        let name = HSTRING::from(if is_dir {
            "folder".to_owned()
        } else {
            format!("file.{key}")
        });
        let get = |extra| unsafe {
            let mut info = SHFILEINFOW::default();
            SHGetFileInfoW(
                &name,
                FILE_FLAGS_AND_ATTRIBUTES(attrs.0),
                Some(&mut info),
                std::mem::size_of::<SHFILEINFOW>() as u32,
                SHGFI_SYSICONINDEX | SHGFI_SMALLICON | SHGFI_USEFILEATTRIBUTES | extra,
            );
            info.iIcon
        };
        let normal = get(Default::default());
        let open = if is_dir { get(SHGFI_OPENICON) } else { normal };
        self.ws.icons.insert(key, (normal, open));
        (normal, open)
    }

    /// 項目を `parent` の下に加える。
    fn insert_node(&mut self, parent: HTREEITEM, node: Node, label: &str) -> HTREEITEM {
        let (image, selected) = self.icon_for(&node.path, node.is_dir);
        let children = i32::from(node.is_dir);
        let index = self.ws.nodes.len();
        self.ws.nodes.push(node);
        let mut text: Vec<u16> = label.encode_utf16().chain(std::iter::once(0)).collect();
        let ins = TVINSERTSTRUCTW {
            hParent: parent,
            hInsertAfter: TVI_LAST,
            Anonymous: TVINSERTSTRUCTW_0 {
                itemex: TVITEMEXW {
                    mask: TVIF_TEXT | TVIF_PARAM | TVIF_CHILDREN | TVIF_IMAGE | TVIF_SELECTEDIMAGE,
                    pszText: windows::core::PWSTR(text.as_mut_ptr()),
                    cChildren: TVITEMEXW_CHILDREN(children),
                    lParam: LPARAM(index as isize),
                    iImage: image,
                    iSelectedImage: selected,
                    ..Default::default()
                },
            },
        };
        unsafe {
            HTREEITEM(
                SendMessageW(
                    self.ws.tree,
                    TVM_INSERTITEMW,
                    None,
                    Some(LPARAM(&ins as *const _ as isize)),
                )
                .0,
            )
        }
    }

    /// ツリーを作り直す（起点のフォルダだけを並べる）。
    pub(crate) fn rebuild_tree(&mut self) {
        unsafe {
            SendMessageW(self.ws.tree, WM_SETREDRAW, Some(WPARAM(0)), None);
            SendMessageW(self.ws.tree, TVM_DELETEITEM, None, Some(LPARAM(TVI_ROOT.0)));
        }
        self.ws.nodes.clear();
        let folders = self.ws.workspace.folders.clone();
        for f in folders {
            // 同じ名前の起点があれば親のフォルダも示す
            let name = display_name(&f);
            let dup = self
                .ws
                .workspace
                .folders
                .iter()
                .filter(|g| display_name(g).eq_ignore_ascii_case(&name))
                .count()
                > 1;
            let label = match (dup, f.parent()) {
                (true, Some(p)) => format!("{name}（{}）", p.display()),
                _ => name,
            };
            self.insert_node(
                TVI_ROOT,
                Node {
                    path: f,
                    is_dir: true,
                    loaded: false,
                    root: true,
                },
                &label,
            );
        }
        unsafe {
            SendMessageW(self.ws.tree, WM_SETREDRAW, Some(WPARAM(1)), None);
            let _ = windows::Win32::Graphics::Gdi::InvalidateRect(Some(self.ws.tree), None, true);
        }
    }

    /// 項目の番号（lParam）。
    fn node_of(&self, item: HTREEITEM) -> Option<usize> {
        let mut tv = TVITEMEXW {
            mask: TVIF_PARAM | TVIF_HANDLE,
            hItem: item,
            ..Default::default()
        };
        let ok = unsafe {
            SendMessageW(
                self.ws.tree,
                TVM_GETITEMW,
                None,
                Some(LPARAM(&mut tv as *mut _ as isize)),
            )
            .0
        };
        (ok != 0 && (tv.lParam.0 as usize) < self.ws.nodes.len()).then_some(tv.lParam.0 as usize)
    }

    /// フォルダの項目の中身を読む（まだなら）。
    fn load_children(&mut self, item: HTREEITEM) {
        let Some(i) = self.node_of(item) else { return };
        if self.ws.nodes[i].loaded || !self.ws.nodes[i].is_dir {
            return;
        }
        self.ws.nodes[i].loaded = true;
        let dir = self.ws.nodes[i].path.clone();
        let (entries, skipped) = match workspace::list_dir(&dir) {
            Ok(v) => v,
            Err(e) => {
                self.status_msg = format!("{} を読めません: {e}", dir.display());
                self.update_status();
                (Vec::new(), 0)
            }
        };
        if entries.is_empty() {
            // 中身がなければ展開のボタンを消す
            let tv = TVITEMEXW {
                mask: TVIF_CHILDREN | TVIF_HANDLE,
                hItem: item,
                cChildren: TVITEMEXW_CHILDREN(0),
                ..Default::default()
            };
            unsafe {
                SendMessageW(
                    self.ws.tree,
                    TVM_SETITEMW,
                    None,
                    Some(LPARAM(&tv as *const _ as isize)),
                );
            }
            return;
        }
        unsafe {
            SendMessageW(self.ws.tree, WM_SETREDRAW, Some(WPARAM(0)), None);
        }
        for e in entries {
            self.insert_node(
                item,
                Node {
                    path: e.path,
                    is_dir: e.is_dir,
                    loaded: false,
                    root: false,
                },
                &e.name,
            );
        }
        if skipped > 0 {
            self.status_msg = format!(
                "{} の項目が多いため、{} 件を表示していません",
                dir.display(),
                group_digits(skipped as u64)
            );
            self.update_status();
        }
        unsafe {
            SendMessageW(self.ws.tree, WM_SETREDRAW, Some(WPARAM(1)), None);
        }
    }

    /// フォルダの項目を読み直す（開いていれば開いたまま）。
    fn refresh_item(&mut self, item: HTREEITEM) {
        let Some(i) = self.node_of(item) else { return };
        if !self.ws.nodes[i].is_dir {
            return;
        }
        unsafe {
            // 子の項目を消す
            loop {
                let child = SendMessageW(
                    self.ws.tree,
                    TVM_GETNEXTITEM,
                    Some(WPARAM(TVGN_CHILD as usize)),
                    Some(LPARAM(item.0)),
                )
                .0;
                if child == 0 {
                    break;
                }
                SendMessageW(self.ws.tree, TVM_DELETEITEM, None, Some(LPARAM(child)));
            }
            let tv = TVITEMEXW {
                mask: TVIF_CHILDREN | TVIF_HANDLE,
                hItem: item,
                cChildren: TVITEMEXW_CHILDREN(1),
                ..Default::default()
            };
            SendMessageW(
                self.ws.tree,
                TVM_SETITEMW,
                None,
                Some(LPARAM(&tv as *const _ as isize)),
            );
        }
        self.ws.nodes[i].loaded = false;
        let state = unsafe {
            SendMessageW(
                self.ws.tree,
                TVM_GETITEMSTATE,
                Some(WPARAM(item.0 as usize)),
                Some(LPARAM(TVIS_EXPANDED.0 as isize)),
            )
            .0
        };
        if state & TVIS_EXPANDED.0 as isize != 0 {
            self.load_children(item);
        }
    }

    /// 選択している項目。
    fn selected_item(&self) -> HTREEITEM {
        HTREEITEM(unsafe {
            SendMessageW(
                self.ws.tree,
                TVM_GETNEXTITEM,
                Some(WPARAM(TVGN_CARET as usize)),
                None,
            )
            .0
        })
    }

    /// ワークスペースを保存し、最後に使ったものとして記録する。
    fn save_workspace(&mut self) {
        let file = match &self.ws.file {
            Some(f) => f.clone(),
            None => match workspace::config_file(workspace::UNTITLED_FILE) {
                Some(f) => f,
                None => return,
            },
        };
        self.ws.file = Some(file.clone());
        if let Err(e) = self.ws.workspace.save(&file) {
            self.status_msg = format!("ワークスペースを保存できません: {e}");
            self.update_status();
            return;
        }
        let _ = workspace::set_last_used(&file);
    }

    /// フォルダをワークスペースに加える。
    pub(crate) fn add_workspace_folder(&mut self, dir: &Path) {
        let dir = crate::recentdlg::normalize(dir);
        if !self.ws.workspace.add(&dir) {
            self.status_msg = format!("{} は既にワークスペースにあります", dir.display());
            self.update_status();
            return;
        }
        self.save_workspace();
        self.rebuild_tree();
        if !self.ws.visible {
            self.toggle_sidebar();
        }
        self.status_msg = format!("{} をワークスペースに加えました", dir.display());
        self.update_status();
    }

    /// ワークスペースを切り替える（`file` が `None` なら名前のない空のワークスペース）。
    fn switch_workspace(&mut self, ws: Workspace, file: Option<PathBuf>) {
        self.ws.workspace = ws;
        self.ws.file = file;
        self.save_workspace();
        self.rebuild_tree();
        // フォルダがあればサイドバーを表示する
        if !self.ws.visible && !self.ws.workspace.folders.is_empty() {
            self.toggle_sidebar();
        }
        self.update_title();
    }

    /// ワークスペースの名前（タイトル用。名前を付けていなければ `None`）。
    pub(crate) fn workspace_title(&self) -> Option<String> {
        if !self.ws.named() {
            return None;
        }
        self.ws
            .file
            .as_deref()
            .and_then(|f| f.file_stem())
            .map(|s| s.to_string_lossy().into_owned())
    }

    /// 項目のパスとフォルダか。
    fn item_info(&self, item: HTREEITEM) -> Option<(PathBuf, bool)> {
        let i = self.node_of(item)?;
        Some((self.ws.nodes[i].path.clone(), self.ws.nodes[i].is_dir))
    }
}

/// ツリーの通知（フレームの WM_NOTIFY）。処理したら戻り値。
pub(crate) fn on_tree_notify(hwnd: HWND, hdr: &NMHDR, lparam: LPARAM) -> Option<LRESULT> {
    let tree = with_app(|a| a.ws.tree)?;
    if hdr.hwndFrom != tree {
        return None;
    }
    match hdr.code {
        TVN_ITEMEXPANDINGW => {
            let nm = unsafe { &*(lparam.0 as *const NMTREEVIEWW) };
            if nm.action == TVE_EXPAND {
                with_app(|a| a.load_children(nm.itemNew.hItem));
            }
            Some(LRESULT(0))
        }
        NM_DBLCLK => {
            let item = with_app(|a| a.selected_item())?;
            let path = with_app(|a| {
                let i = a.node_of(item)?;
                (!a.ws.nodes[i].is_dir).then(|| a.ws.nodes[i].path.clone())
            })
            .flatten();
            match path {
                Some(p) => {
                    open_path(hwnd, p, None, false);
                    Some(LRESULT(1))
                }
                // フォルダは既定の動作（開閉）
                None => Some(LRESULT(0)),
            }
        }
        TVN_KEYDOWN => {
            let nm = unsafe { &*(lparam.0 as *const NMTVKEYDOWN) };
            if nm.wVKey == VK_RETURN.0 {
                let item = with_app(|a| a.selected_item())?;
                match with_app(|a| a.item_info(item)).flatten() {
                    Some((p, false)) => open_path(hwnd, p, None, false),
                    // フォルダは開閉する（展開の通知で中身を読むので、アプリの状態を借りずに送る）
                    Some((_, true)) => unsafe {
                        SendMessageW(
                            tree,
                            TVM_EXPAND,
                            Some(WPARAM(TVE_TOGGLE.0 as usize)),
                            Some(LPARAM(item.0)),
                        );
                    },
                    None => {}
                }
            }
            Some(LRESULT(0))
        }
        NM_RCLICK => {
            context_menu(hwnd, tree);
            Some(LRESULT(1))
        }
        TVN_GETINFOTIPW => {
            let tip = unsafe { &mut *(lparam.0 as *mut NMTVGETINFOTIPW) };
            let path = with_app(|a| {
                a.ws.nodes
                    .get(tip.lParam.0 as usize)
                    .map(|n| n.path.display().to_string())
            })
            .flatten()
            .unwrap_or_default();
            let text: Vec<u16> = path.encode_utf16().collect();
            let n = text.len().min(tip.cchTextMax.max(1) as usize - 1);
            unsafe {
                std::ptr::copy_nonoverlapping(text.as_ptr(), tip.pszText.0, n);
                *tip.pszText.0.add(n) = 0;
            }
            Some(LRESULT(0))
        }
        _ => None,
    }
}

/// 右クリックのメニュー。
fn context_menu(hwnd: HWND, tree: HWND) {
    // カーソルの下の項目を選ぶ
    let mut pt = windows::Win32::Foundation::POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut pt);
    }
    let screen = pt;
    unsafe {
        let _ = windows::Win32::Graphics::Gdi::ScreenToClient(tree, &mut pt);
    }
    let mut hit = TVHITTESTINFO {
        pt,
        ..Default::default()
    };
    let item = HTREEITEM(unsafe {
        SendMessageW(
            tree,
            TVM_HITTEST,
            None,
            Some(LPARAM(&mut hit as *mut _ as isize)),
        )
        .0
    });
    let node = if item.0 != 0 {
        unsafe {
            SendMessageW(
                tree,
                TVM_SELECTITEM,
                Some(WPARAM(TVGN_CARET as usize)),
                Some(LPARAM(item.0)),
            );
        }
        with_app(|a| {
            a.node_of(item).map(|i| {
                let n = &a.ws.nodes[i];
                (n.path.clone(), n.is_dir, n.root)
            })
        })
        .flatten()
    } else {
        None
    };
    let cmd = unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        let add = |id: u32, text: &str| {
            let _ = AppendMenuW(menu, MF_STRING, id as usize, &HSTRING::from(text));
        };
        let sep = || {
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        };
        match &node {
            Some((_, is_dir, root)) => {
                if !is_dir {
                    add(CM_OPEN, "開く(&O)");
                    sep();
                }
                add(CM_TERMINAL, "ターミナルで開く(&T)");
                add(CM_EXPLORER, "エクスプローラーで開く(&E)");
                if *is_dir {
                    add(CM_GREP, "フォルダ内を検索 (Grep)(&F)...");
                }
                sep();
                add(CM_COPY_PATH, "パスをコピー(&C)");
                add(CM_COPY_RELATIVE, "相対パスをコピー(&R)");
                if *is_dir {
                    sep();
                    add(CM_REFRESH, "最新の情報に更新(&U)");
                }
                if *root {
                    add(CM_REMOVE_ROOT, "ワークスペースから削除(&D)");
                }
                sep();
                add(CM_ADD_FOLDER, "フォルダをワークスペースに追加(&A)...");
            }
            None => add(CM_ADD_FOLDER, "フォルダをワークスペースに追加(&A)..."),
        }
        let cmd = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON,
            screen.x,
            screen.y,
            None,
            hwnd,
            None,
        );
        let _ = DestroyMenu(menu);
        cmd.0 as u32
    };
    run_context_command(hwnd, cmd, item, node);
}

fn run_context_command(hwnd: HWND, cmd: u32, item: HTREEITEM, node: Option<(PathBuf, bool, bool)>) {
    if cmd == CM_ADD_FOLDER {
        return cmd_add_folder(hwnd);
    }
    let Some((path, is_dir, _)) = node else {
        return;
    };
    // ファイルの場合はそのフォルダ
    let dir = if is_dir {
        path.clone()
    } else {
        path.parent().map(|p| p.to_owned()).unwrap_or_default()
    };
    let result = match cmd {
        CM_OPEN => {
            open_path(hwnd, path, None, false);
            Ok(())
        }
        CM_TERMINAL => {
            let custom = with_app(|a| a.config.workspace.terminal.clone()).unwrap_or_default();
            open_terminal(&dir, &custom)
        }
        CM_EXPLORER => open_explorer(&path),
        CM_COPY_PATH | CM_COPY_RELATIVE => {
            let text = if cmd == CM_COPY_RELATIVE {
                with_app(|a| {
                    a.ws.workspace
                        .root_of(&path)
                        .and_then(|r| path.strip_prefix(r).ok())
                        .map(|p| p.display().to_string())
                })
                .flatten()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| display_name(&path))
            } else {
                path.display().to_string()
            };
            if clipboard::set_text(hwnd, &text, false) {
                Ok(())
            } else {
                Err("クリップボードにコピーできませんでした".into())
            }
        }
        CM_GREP => {
            let Some(mut initial) = with_app(|a| a.grep_defaults()) else {
                return;
            };
            initial.dir = dir.display().to_string();
            if let Some(req) = crate::grepdlg::prompt(hwnd, &initial) {
                with_app(|a| a.start_grep(req));
            }
            Ok(())
        }
        CM_REFRESH => {
            with_app(|a| a.refresh_item(item));
            Ok(())
        }
        CM_REMOVE_ROOT => {
            with_app(|a| {
                a.ws.workspace.remove(&path);
                a.save_workspace();
                a.rebuild_tree();
            });
            Ok(())
        }
        _ => Ok(()),
    };
    if let Err(e) = result {
        error_box(hwnd, &e);
    }
}

/// 「フォルダをワークスペースに追加」。
pub(crate) fn cmd_add_folder(hwnd: HWND) {
    if let Some(dir) = crate::grepdlg::browse_folder(hwnd) {
        with_app(|a| a.add_workspace_folder(&dir));
    }
}

/// ワークスペースのファイルを選ぶ（`save` なら保存先）。
fn pick_workspace_file(owner: HWND, save: bool, current: Option<&Path>) -> Option<PathBuf> {
    use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
    unsafe {
        let filters = [COMDLG_FILTERSPEC {
            pszName: w!("yyeditor のワークスペース (*.yyworkspace)"),
            pszSpec: w!("*.yyworkspace"),
        }];
        let item = if save {
            let d: IFileSaveDialog =
                CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER).ok()?;
            let _ = d.SetFileTypes(&filters);
            let _ = d.SetDefaultExtension(w!("yyworkspace"));
            let name = current
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "workspace.yyworkspace".to_owned());
            let _ = d.SetFileName(&HSTRING::from(name));
            d.Show(Some(owner)).ok()?;
            d.GetResult().ok()?
        } else {
            let d: IFileOpenDialog =
                CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
            let _ = d.SetFileTypes(&filters);
            d.Show(Some(owner)).ok()?;
            d.GetResult().ok()?
        };
        let name = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let path = name.to_string().ok();
        CoTaskMemFree(Some(name.0 as *const _));
        path.map(PathBuf::from)
    }
}

/// ワークスペースのメニューの項目。処理したら `true`。
pub(crate) fn on_workspace_command(hwnd: HWND, id: u16) -> bool {
    match id {
        ID_WS_SIDEBAR => {
            with_app(|a| a.toggle_sidebar());
        }
        ID_WS_ADD_FOLDER => cmd_add_folder(hwnd),
        ID_WS_NEW => {
            with_app(|a| a.switch_workspace(Workspace::default(), None));
        }
        ID_WS_OPEN => {
            if let Some(file) = pick_workspace_file(hwnd, false, None) {
                match Workspace::load(&file) {
                    Ok(ws) => {
                        with_app(|a| a.switch_workspace(ws, Some(file)));
                    }
                    Err(e) => error_box(
                        hwnd,
                        &format!("ワークスペースを開けません。\n{}\n\n{e}", file.display()),
                    ),
                }
            }
        }
        ID_WS_SAVE_AS => {
            let current = with_app(|a| a.ws.named().then(|| a.ws.file.clone()).flatten()).flatten();
            if let Some(file) = pick_workspace_file(hwnd, true, current.as_deref()) {
                with_app(|a| {
                    a.ws.file = Some(file);
                    a.save_workspace();
                    a.update_title();
                    a.status_msg = "ワークスペースを保存しました".into();
                    a.update_status();
                });
            }
        }
        ID_WS_REFRESH => {
            with_app(|a| a.rebuild_tree());
        }
        _ => return false,
    }
    true
}

/// ワークスペースのメニュー。
pub(crate) fn create_workspace_menu() -> Result<HMENU> {
    unsafe {
        let m = CreatePopupMenu()?;
        let item =
            |id: u16, text: windows::core::PCWSTR| AppendMenuW(m, MF_STRING, id as usize, text);
        item(
            ID_WS_SIDEBAR,
            w!("サイドバー（エクスプローラー）(&E)\tCtrl+Shift+E"),
        )?;
        AppendMenuW(m, MF_SEPARATOR, 0, None)?;
        item(
            ID_WS_ADD_FOLDER,
            w!("フォルダをワークスペースに追加(&A)..."),
        )?;
        item(ID_WS_REFRESH, w!("最新の情報に更新(&U)"))?;
        AppendMenuW(m, MF_SEPARATOR, 0, None)?;
        item(ID_WS_NEW, w!("新しいワークスペース(&N)"))?;
        item(ID_WS_OPEN, w!("ワークスペースを開く(&O)..."))?;
        item(ID_WS_SAVE_AS, w!("ワークスペースに名前を付けて保存(&S)..."))?;
        Ok(m)
    }
}
