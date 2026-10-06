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
pub(crate) const ID_WS_ADD_REMOTE: u16 = 1306;
/// リモートのフォルダの中身を読む（`WPARAM` はツリーの世代、`LPARAM` は項目の番号）。
/// 展開の通知の中で接続・取得（メッセージを処理しながら待つ）をしないよう、後で行う
pub(crate) const WM_APP_WS_REMOTE: u32 = WM_APP + 32;
/// ファイル・フォルダの操作（`LPARAM` は `Box<WsOp>`）。ツリーの通知の中では行わず、後で行う
pub(crate) const WM_APP_WS_OP: u32 = WM_APP + 33;
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
const CM_ADD_REMOTE: u32 = 10;
const CM_NEW_FILE: u32 = 11;
const CM_NEW_FOLDER: u32 = 12;
const CM_RENAME: u32 = 13;
const CM_DELETE: u32 = 14;
const CM_CUT: u32 = 15;
const CM_PASTE: u32 = 16;
const CM_COPY: u32 = 17;

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
    /// ツリーの項目
    item: HTREEITEM,
    /// ツリーに残っている（フォルダを読み直すと、中の項目は消える）
    alive: bool,
}

impl Node {
    /// SSH 接続先のフォルダ・ファイル（パスは `ssh://…`）か。
    fn remote(&self) -> Option<yy_remote::RemoteUri> {
        remote_uri(&self.path)
    }
}

/// `ssh://…` のパスなら、その場所。
fn remote_uri(path: &Path) -> Option<yy_remote::RemoteUri> {
    path.to_str().and_then(yy_remote::RemoteUri::parse)
}

/// コピー・切り取りしたもの。
#[derive(Clone)]
struct Clip {
    path: PathBuf,
    /// 切り取り（貼り付けで移動する）。でなければコピー
    cut: bool,
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
    /// ツリーを作り直すたびに増える（作り直す前に頼んだリモートの読み込みを捨てる）
    generation: usize,
    /// コピー・切り取りしたファイル・フォルダ（貼り付けでコピー・移動する）
    clip: Option<Clip>,
    /// ドラッグ中の項目の番号
    drag: Option<usize>,
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
                        | TVS_INFOTIP
                        | TVS_EDITLABELS,
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
        generation: 0,
        clip: None,
        drag: None,
    })
}

/// 実行ファイルと同じフォルダの yyterm（12 章。なければ `None`）。
pub(crate) fn yyterm_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.parent()?.join("yyterm.exe");
    exe.is_file().then_some(exe)
}

/// フォルダをターミナルで開く。`custom` は設定のコマンド（空なら yyterm、なければ Windows Terminal、
/// なければコマンド プロンプト）。リモートのフォルダ（`ssh://…`）は yyterm だけで開ける。
pub(crate) fn open_terminal(dir: &Path, custom: &[String]) -> std::result::Result<(), String> {
    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
    if remote_uri(dir).is_some() || custom.is_empty() {
        if let Some(t) = yyterm_exe() {
            return Command::new(&t)
                .arg(dir)
                .spawn()
                .map(|_| ())
                .map_err(|e| format!("{}: {e}", t.display()));
        }
        if remote_uri(dir).is_some() {
            return Err("リモートのフォルダは yyterm（yyterm.exe）で開きます。".into());
        }
    }
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
        let item = unsafe {
            HTREEITEM(
                SendMessageW(
                    self.ws.tree,
                    TVM_INSERTITEMW,
                    None,
                    Some(LPARAM(&ins as *const _ as isize)),
                )
                .0,
            )
        };
        self.ws.nodes[index].item = item;
        item
    }

    /// ツリーを作り直す（起点のフォルダだけを並べる）。
    pub(crate) fn rebuild_tree(&mut self) {
        unsafe {
            SendMessageW(self.ws.tree, WM_SETREDRAW, Some(WPARAM(0)), None);
            SendMessageW(self.ws.tree, TVM_DELETEITEM, None, Some(LPARAM(TVI_ROOT.0)));
        }
        self.ws.nodes.clear();
        self.ws.generation += 1;
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
            let label = match (remote_uri(&f), dup, f.parent()) {
                // リモートのフォルダは接続先も示す
                (Some(u), _, _) => format!("{name} [{}]", u.target()),
                (None, true, Some(p)) => format!("{name}（{}）", p.display()),
                _ => name,
            };
            self.insert_node(
                TVI_ROOT,
                Node {
                    path: f,
                    is_dir: true,
                    loaded: false,
                    root: true,
                    item: HTREEITEM::default(),
                    alive: true,
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

    /// フォルダの項目の中身を読む（まだなら）。リモートのフォルダは後で読む
    /// （[`WM_APP_WS_REMOTE`]）ので、そのときは `true` を返す（展開はそのあとで行う）。
    fn load_children(&mut self, item: HTREEITEM) -> bool {
        let Some(i) = self.node_of(item) else {
            return false;
        };
        if self.ws.nodes[i].loaded || !self.ws.nodes[i].is_dir {
            return false;
        }
        if self.ws.nodes[i].remote().is_some() {
            unsafe {
                let _ = PostMessageW(
                    Some(self.frame),
                    WM_APP_WS_REMOTE,
                    WPARAM(self.ws.generation),
                    LPARAM(i as isize),
                );
            }
            return true;
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
        self.insert_children(item, &dir, entries, skipped);
        false
    }

    /// 読んだフォルダの中身を `item` の下に並べる。
    fn insert_children(
        &mut self,
        item: HTREEITEM,
        dir: &Path,
        entries: Vec<workspace::Entry>,
        skipped: usize,
    ) {
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
                    item: HTREEITEM::default(),
                    alive: true,
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
        // 消した子の項目は使わない
        let dir = self.ws.nodes[i].path.clone();
        for n in &mut self.ws.nodes {
            if n.path != dir && workspace::is_within(&n.path, &dir) {
                n.alive = false;
            }
        }
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
            let _ = self.load_children(item);
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

/// [`WM_APP_WS_REMOTE`]: リモートのフォルダの中身を読み（接続していなければ接続し）、展開する。
pub(crate) fn load_remote(hwnd: HWND, generation: usize, index: usize) {
    let target = with_app(|a| {
        if a.ws.generation != generation {
            return None;
        }
        let n = a.ws.nodes.get(index)?;
        if n.loaded || !n.alive {
            return None;
        }
        Some((n.remote()?, n.item, n.path.clone()))
    })
    .flatten();
    let Some((uri, item, dir)) = target else {
        return;
    };
    // 読んでいる間はアプリの状態を借りない（接続中の問い合わせのダイアログが出るため）
    let listed = crate::remote::list_dir(&uri);
    let expand = with_app(|a| {
        // 待っている間にツリーが作り直されていれば捨てる
        if a.ws.generation != generation || a.ws.nodes.get(index).is_none_or(|n| n.loaded) {
            return Ok(false);
        }
        let (entries, skipped) = listed?;
        a.ws.nodes[index].loaded = true;
        a.insert_children(item, &dir, entries, skipped);
        Ok::<_, String>(true)
    });
    match expand {
        Some(Ok(true)) => unsafe {
            let tree = with_app(|a| a.ws.tree).unwrap_or_default();
            SendMessageW(
                tree,
                TVM_EXPAND,
                Some(WPARAM(TVE_EXPAND.0 as usize)),
                Some(LPARAM(item.0)),
            );
        },
        Some(Err(e)) => error_box(hwnd, &format!("フォルダを開けません。\n{e}")),
        _ => {}
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
            if nm.action == TVE_EXPAND
                && with_app(|a| a.load_children(nm.itemNew.hItem)) == Some(true)
            {
                // リモートのフォルダは読んでから展開する
                return Some(LRESULT(1));
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
            if nm.wVKey == VK_F2.0 || nm.wVKey == VK_DELETE.0 {
                let item = with_app(|a| a.selected_item())?;
                let node = with_app(|a| {
                    let i = a.node_of(item)?;
                    let n = &a.ws.nodes[i];
                    (!n.root).then(|| (n.path.clone(), n.is_dir))
                })
                .flatten();
                if let Some((path, is_dir)) = node {
                    if nm.wVKey == VK_F2.0 {
                        unsafe {
                            SendMessageW(tree, TVM_EDITLABELW, None, Some(LPARAM(item.0)));
                        }
                    } else {
                        post_op(hwnd, WsOp::Delete { path, is_dir });
                    }
                }
                return Some(LRESULT(0));
            }
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
        TVN_BEGINLABELEDITW => {
            // 起点のフォルダの名前は変えない（ワークスペースから外して加え直す）
            let info = unsafe { &*(lparam.0 as *const NMTVDISPINFOW) };
            let root = with_app(|a| a.ws.nodes.get(info.item.lParam.0 as usize).map(|n| n.root))
                .flatten()
                .unwrap_or(true);
            Some(LRESULT(isize::from(root)))
        }
        TVN_ENDLABELEDITW => {
            let info = unsafe { &*(lparam.0 as *const NMTVDISPINFOW) };
            if !info.item.pszText.is_null() {
                let name = unsafe { info.item.pszText.to_string() }.unwrap_or_default();
                let path = with_app(|a| {
                    a.ws.nodes
                        .get(info.item.lParam.0 as usize)
                        .map(|n| n.path.clone())
                })
                .flatten();
                if let Some(path) = path {
                    post_op(hwnd, WsOp::Rename { path, name });
                }
            }
            // 表示は名前を変えたあとに読み直して直す
            Some(LRESULT(0))
        }
        TVN_BEGINDRAGW => {
            let nm = unsafe { &*(lparam.0 as *const NMTREEVIEWW) };
            let index = nm.itemNew.lParam.0 as usize;
            let ok = with_app(|a| {
                let movable = a.ws.nodes.get(index).is_some_and(|n| n.alive);
                if movable {
                    a.ws.drag = Some(index);
                }
                movable
            });
            if ok == Some(true) {
                unsafe {
                    SetCapture(hwnd);
                }
            }
            Some(LRESULT(0))
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
    let clip_pending = with_app(|a| a.ws.clip.is_some()).unwrap_or(false);
    let cmd = unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        let add = |id: u32, text: &str| {
            let _ = AppendMenuW(menu, MF_STRING, id as usize, &HSTRING::from(text));
        };
        let sep = || {
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        };
        match &node {
            Some((path, is_dir, root)) => {
                // リモートの項目には、手元のプログラムで開く操作と Grep はない
                let local = remote_uri(path).is_none();
                if !is_dir {
                    add(CM_OPEN, "開く(&O)");
                    sep();
                }
                if *is_dir {
                    add(CM_NEW_FILE, "新しいファイル(&W)...");
                    add(CM_NEW_FOLDER, "新しいフォルダ(&N)...");
                    sep();
                }
                if !root {
                    add(CM_CUT, "切り取り(&X)\tCtrl+X");
                }
                add(CM_COPY, "コピー(&C)\tCtrl+C");
                if *is_dir && clip_pending {
                    add(CM_PASTE, "貼り付け(&P)\tCtrl+V");
                }
                if !root {
                    add(CM_RENAME, "名前の変更(&M)\tF2");
                    add(CM_DELETE, "削除(&D)\tDel");
                }
                sep();
                if local {
                    add(CM_TERMINAL, "ターミナルで開く(&T)");
                    add(CM_EXPLORER, "エクスプローラーで開く(&E)");
                    if *is_dir {
                        add(CM_GREP, "フォルダ内を検索 (Grep)(&F)...");
                    }
                    sep();
                } else if yyterm_exe().is_some() {
                    // リモートのフォルダは yyterm の SSH で開く
                    add(CM_TERMINAL, "ターミナルで開く(&T)");
                    sep();
                }
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
                add(
                    CM_ADD_REMOTE,
                    "リモートのフォルダをワークスペースに追加(&S)...",
                );
            }
            None => {
                add(CM_ADD_FOLDER, "フォルダをワークスペースに追加(&A)...");
                add(
                    CM_ADD_REMOTE,
                    "リモートのフォルダをワークスペースに追加(&S)...",
                );
            }
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
    if cmd == CM_ADD_REMOTE {
        return cmd_add_remote_folder(hwnd);
    }
    let Some((path, is_dir, _)) = node else {
        return;
    };
    match cmd {
        CM_NEW_FILE => return post_op(hwnd, WsOp::NewFile(path)),
        CM_NEW_FOLDER => return post_op(hwnd, WsOp::NewFolder(path)),
        CM_DELETE => return post_op(hwnd, WsOp::Delete { path, is_dir }),
        CM_CUT => return set_clip(path, true),
        CM_COPY => return set_clip(path, false),
        CM_PASTE => return paste_into(hwnd, path, is_dir),
        CM_RENAME => {
            if let Some(tree) = with_app(|a| a.ws.tree) {
                unsafe {
                    SendMessageW(tree, TVM_EDITLABELW, None, Some(LPARAM(item.0)));
                }
            }
            return;
        }
        _ => {}
    }
    // ファイルの場合はそのフォルダ
    let dir = if is_dir {
        path.clone()
    } else {
        workspace::parent(&path).unwrap_or_default()
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
            let text = if let Some(u) = remote_uri(&path) {
                // リモートは接続先のパス（相対パスは起点のフォルダから。区切りは /）
                let root = with_app(|a| a.ws.workspace.root_of(&path).map(|r| r.to_owned()))
                    .flatten()
                    .and_then(|r| remote_uri(&r));
                let relative = root.as_ref().and_then(|r| {
                    let rest = u.path.strip_prefix(r.path.as_slice())?;
                    let rest = rest.strip_prefix(b"/").unwrap_or(rest);
                    (!rest.is_empty()).then(|| yy_proto::display_path(rest))
                });
                if cmd == CM_COPY_RELATIVE {
                    relative.unwrap_or_else(|| display_name(&path))
                } else {
                    yy_proto::display_path(&u.path)
                }
            } else if cmd == CM_COPY_RELATIVE {
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

/// 「リモートのフォルダをワークスペースに追加」。
pub(crate) fn cmd_add_remote_folder(hwnd: HWND) {
    let Some((available, initial)) = with_app(|a| {
        // 選択中のリモートの項目、なければ最後に使った場所から
        let selected = a
            .node_of(a.selected_item())
            .and_then(|i| a.ws.nodes[i].remote());
        (
            crate::remote::available(),
            selected.or_else(crate::remote::last),
        )
    }) else {
        return;
    };
    if !available {
        info_box(
            hwnd,
            "この yyeditor には SSH の機能が組み込まれていません。",
        );
        return;
    }
    let initial = initial.map(|mut u| {
        // ファイルならそのフォルダから
        if !u.path.ends_with(b"/") {
            u.path = yy_proto::parent_path(&u.path);
        }
        u
    });
    if let Some(p) =
        crate::remotedlg::show(hwnd, crate::remotedlg::Mode::Folder, initial, None, false)
    {
        let dir = PathBuf::from(p.uri.to_string());
        with_app(|a| {
            crate::remote::set_last(p.uri.clone());
            a.add_workspace_folder(&dir);
        });
    }
}

/// ワークスペースのファイルを選ぶ（`save` なら保存先）。
pub(crate) fn pick_workspace_file(
    owner: HWND,
    save: bool,
    current: Option<&Path>,
) -> Option<PathBuf> {
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
    // サイドバーで名前を変えている間の編集のショートカットは、その入力欄で行う
    let label_edit = with_app(|a| unsafe {
        HWND(SendMessageW(a.ws.tree, TVM_GETEDITCONTROL, None, None).0 as *mut _)
    })
    .filter(|e| !e.0.is_null() && unsafe { GetFocus() } == *e);
    if let Some(edit) = label_edit {
        let msg = match id {
            ID_CUT => Some((WM_CUT, 0, 0)),
            ID_COPY => Some((WM_COPY, 0, 0)),
            ID_PASTE => Some((WM_PASTE, 0, 0)),
            ID_UNDO => Some((WM_UNDO, 0, 0)),
            ID_SELECT_ALL => Some((windows::Win32::UI::Controls::EM_SETSEL, 0, -1)),
            _ => None,
        };
        if let Some((m, w, l)) = msg {
            unsafe {
                SendMessageW(edit, m, Some(WPARAM(w)), Some(LPARAM(l)));
            }
            return true;
        }
    }
    // サイドバーにフォーカスがあれば、切り取り・貼り付けはファイル・フォルダの操作
    if matches!(id, ID_CUT | ID_COPY | ID_PASTE)
        && with_app(|a| unsafe { GetFocus() } == a.ws.tree) == Some(true)
    {
        let node = with_app(|a| {
            let i = a.node_of(a.selected_item())?;
            let n = &a.ws.nodes[i];
            Some((n.path.clone(), n.is_dir, n.root))
        })
        .flatten();
        match (id, node) {
            (ID_CUT, Some((path, _, false))) => set_clip(path, true),
            (ID_COPY, Some((path, _, _))) => set_clip(path, false),
            (ID_PASTE, Some((path, is_dir, _))) => paste_into(hwnd, path, is_dir),
            _ => {}
        }
        return true;
    }
    match id {
        ID_WS_SIDEBAR => {
            with_app(|a| a.toggle_sidebar());
        }
        ID_WS_ADD_FOLDER => cmd_add_folder(hwnd),
        ID_WS_ADD_REMOTE => cmd_add_remote_folder(hwnd),
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
        item(
            ID_WS_ADD_REMOTE,
            w!("リモートのフォルダをワークスペースに追加(&S)..."),
        )?;
        item(ID_WS_REFRESH, w!("最新の情報に更新(&U)"))?;
        AppendMenuW(m, MF_SEPARATOR, 0, None)?;
        item(ID_WS_NEW, w!("新しいワークスペース(&N)"))?;
        item(ID_WS_OPEN, w!("ワークスペースを開く(&O)..."))?;
        item(ID_WS_SAVE_AS, w!("ワークスペースに名前を付けて保存(&S)..."))?;
        Ok(m)
    }
}

// ---- ファイル・フォルダの操作 -------------------------------------------------------
//
// 作成・名前の変更・削除・移動を、手元のフォルダでもリモート（`ssh://`）のフォルダでも行う。
// 移動は同じ場所（同じパソコン、または同じ接続先）の中だけ。手元の削除はごみ箱へ移す。
// リモートの操作は接続中のダイアログや待ちを伴うので、ツリーの通知の中では行わず
// [`WM_APP_WS_OP`] で後で行う。

/// サイドバーでのファイル・フォルダの操作。
pub(crate) enum WsOp {
    /// フォルダの中に新しいフォルダを作る（名前を尋ねる）
    NewFolder(PathBuf),
    /// フォルダの中に新しいファイルを作って開く（名前を尋ねる）
    NewFile(PathBuf),
    Rename {
        path: PathBuf,
        name: String,
    },
    Delete {
        path: PathBuf,
        is_dir: bool,
    },
    /// `from` を `to_dir` の中へ移す
    Move {
        from: PathBuf,
        to_dir: PathBuf,
    },
    /// `from` を `to_dir` の中へコピーする（手元とリモートの間も）
    Copy {
        from: PathBuf,
        to_dir: PathBuf,
    },
}

/// 操作を後で行うよう頼む。
fn post_op(frame: HWND, op: WsOp) {
    let boxed = Box::into_raw(Box::new(op));
    unsafe {
        if PostMessageW(Some(frame), WM_APP_WS_OP, WPARAM(0), LPARAM(boxed as isize)).is_err() {
            drop(Box::from_raw(boxed));
        }
    }
}

/// [`WM_APP_WS_OP`] を処理する（フレームのウィンドウプロシージャから）。
pub(crate) fn on_op(hwnd: HWND, lparam: LPARAM) {
    let op = unsafe { *Box::from_raw(lparam.0 as *mut WsOp) };
    let r = match op {
        WsOp::NewFolder(dir) => new_item(hwnd, &dir, true),
        WsOp::NewFile(dir) => new_item(hwnd, &dir, false),
        WsOp::Rename { path, name } => rename_item(&path, name.trim()),
        WsOp::Delete { path, is_dir } => delete_item(hwnd, &path, is_dir),
        WsOp::Move { from, to_dir } => move_item(&from, &to_dir),
        WsOp::Copy { from, to_dir } => copy_item(hwnd, &from, &to_dir),
    };
    if let Err(e) = r {
        error_box(hwnd, &e);
    }
}

/// コピー・切り取りする（貼り付けでコピー・移動する）。
fn set_clip(path: PathBuf, cut: bool) {
    with_app(|a| {
        let what = if cut {
            "切り取りました"
        } else {
            "コピーしました"
        };
        a.status_msg = format!(
            "{} を{what}（貼り付ける先のフォルダで貼り付け）",
            workspace::name_of(&path)
        );
        a.ws.clip = Some(Clip { path, cut });
        a.update_status();
    });
}

/// コピー・切り取りしたものを `path`（ファイルならそのフォルダ）にコピー・移動する。
fn paste_into(hwnd: HWND, path: PathBuf, is_dir: bool) {
    let Some(Some(clip)) = with_app(|a| a.ws.clip.clone()) else {
        return;
    };
    let to_dir = if is_dir {
        path
    } else {
        match workspace::parent(&path) {
            Some(p) => p,
            None => return,
        }
    };
    let from = clip.path;
    post_op(
        hwnd,
        if clip.cut {
            WsOp::Move { from, to_dir }
        } else {
            WsOp::Copy { from, to_dir }
        },
    );
}

fn with_path(path: &Path, e: impl std::fmt::Display) -> String {
    format!("{}\n\n{e}", path.display())
}

/// `path` があるか。
fn exists(path: &Path) -> std::result::Result<bool, String> {
    match remote_uri(path) {
        Some(u) => {
            let p = u.path.clone();
            crate::remote::file_op(&u, "", move |s| match s.stat(&p) {
                Ok(_) => Ok(true),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
                Err(e) => Err(e),
            })
        }
        None => Ok(std::fs::symlink_metadata(path).is_ok()),
    }
}

/// 新しいフォルダ・ファイルを `dir` の中に作る。
fn new_item(hwnd: HWND, dir: &Path, folder: bool) -> std::result::Result<(), String> {
    let remote = remote_uri(dir).is_some();
    let (title, prompt, initial) = if folder {
        ("新しいフォルダ", "フォルダの名前:", "新しいフォルダ")
    } else {
        ("新しいファイル", "ファイルの名前:", "新しいファイル.txt")
    };
    let Some(name) = crate::goto::prompt_text(hwnd, title, prompt, initial) else {
        return Ok(());
    };
    let name = name.trim();
    workspace::check_name(name, remote)?;
    let target = workspace::child(dir, name);
    match remote_uri(&target) {
        Some(u) => {
            let p = u.path.clone();
            crate::remote::file_op(&u, "作成しています…", move |s| {
                if folder {
                    s.make_dir(&p)
                } else {
                    s.create_file(&p).map(|_| ())
                }
            })?;
        }
        None => {
            let r = if folder {
                std::fs::create_dir(&target)
            } else {
                std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&target)
                    .map(|_| ())
            };
            r.map_err(|e| with_path(&target, e))?;
        }
    }
    with_app(|a| a.refresh_folder(dir));
    if !folder {
        open_path(hwnd, target, None, false);
    }
    Ok(())
}

/// 名前を変える。
fn rename_item(path: &Path, name: &str) -> std::result::Result<(), String> {
    if name == workspace::name_of(path) {
        return Ok(());
    }
    workspace::check_name(name, remote_uri(path).is_some())?;
    let Some(dir) = workspace::parent(path) else {
        return Ok(());
    };
    let to = workspace::child(&dir, name);
    // 大文字・小文字だけを変える場合（Windows では同じファイル）は、あるかを確かめない
    if !yy_config::recent::same_path(path, &to) && exists(&to)? {
        return Err(format!("「{name}」は既にあります。"));
    }
    move_path(path, &to)
}

/// `from` を `to_dir` の中へ移す。
fn move_item(from: &Path, to_dir: &Path) -> std::result::Result<(), String> {
    if !workspace::same_place(from, to_dir) {
        return Err(
            "パソコンとリモートの間や、別の接続先へは移動できません（同じ場所の中だけ移動できます）。\n\
             コピーして貼り付けてください。"
                .into(),
        );
    }
    let root = with_app(|a| {
        a.ws.workspace
            .folders
            .iter()
            .any(|f| yy_config::recent::same_path(f, from))
    });
    if root == Some(true) {
        return Err("ワークスペースの起点のフォルダは移動できません（コピーはできます）。".into());
    }
    if workspace::is_within(to_dir, from) {
        return Err("フォルダを、そのフォルダの中へは移動できません。".into());
    }
    if workspace::parent(from).is_some_and(|p| yy_config::recent::same_path(&p, to_dir)) {
        return Ok(());
    }
    let to = workspace::child(to_dir, &workspace::name_of(from));
    if exists(&to)? {
        return Err(format!(
            "移動先に「{}」が既にあります。",
            workspace::name_of(from)
        ));
    }
    move_path(from, &to)
}

/// `path` がフォルダか。
fn is_dir_path(path: &Path) -> std::result::Result<bool, String> {
    match remote_uri(path) {
        Some(u) => {
            let p = u.path.clone();
            crate::remote::file_op(&u, "", move |s| s.stat(&p).map(|i| i.is_dir()))
        }
        None => std::fs::metadata(path)
            .map(|m| m.is_dir())
            .map_err(|e| with_path(path, e)),
    }
}

/// コピー元・コピー先の場所（リモートなら接続する）。
fn loc_of(path: &Path) -> std::result::Result<yy_remote::transfer::Loc, String> {
    use yy_remote::transfer::Loc;
    Ok(match remote_uri(path) {
        Some(u) => Loc::Remote(
            crate::remote::session(&u.target(), &crate::remote::show_status)?,
            u.path,
        ),
        None => Loc::Local(path.to_owned()),
    })
}

/// この大きさ以上のコピーは、始める前に確かめる
const COPY_WARN_BYTES: u64 = 10 << 20;
/// この大きさ以上は、このパソコンとリモートの間（別の接続先の間も）ではコピーしない
const COPY_REMOTE_LIMIT: u64 = 50 << 20;

/// `from` を `to_dir` の中へコピーする。同じ名前があれば「名前 - コピー」にする。
fn copy_item(hwnd: HWND, from: &Path, to_dir: &Path) -> std::result::Result<(), String> {
    if workspace::same_place(from, to_dir) && workspace::is_within(to_dir, from) {
        return Err("フォルダを、そのフォルダの中へはコピーできません。".into());
    }
    let is_dir = is_dir_path(from)?;
    let base = workspace::name_of(from);
    let mut target = None;
    for n in 0..1000 {
        let cand = workspace::child(to_dir, &workspace::copy_name(&base, n, is_dir));
        if !exists(&cand)? {
            target = Some(cand);
            break;
        }
    }
    let Some(target) = target else {
        return Err(format!("「{base}」のコピーの名前を決められませんでした。"));
    };
    let (src, dst) = (loc_of(from)?, loc_of(&target)?);
    let cross = yy_remote::transfer::crosses_network(&src, &dst);
    // 始める前に量を数え、大きければ確かめる（リモートとの間で大きすぎればコピーしない）
    crate::remote::show_status(&format!("{base} の大きさを調べています…（Esc で中止）"));
    let s2 = src.clone();
    let measured = crate::remote::wait(&crate::remote::show_status, move |work| {
        yy_remote::transfer::measure(&s2, &mut |st| {
            work.report(format!(
                "大きさを調べています… {} 個のファイル、{}（Esc で中止）",
                group_digits(st.files),
                crate::util::human_size(st.bytes)
            ));
            !work.cancelled()
        })
    });
    crate::remote::show_status("");
    let size = match measured {
        Ok(st) => st,
        Err(e) if yy_remote::transfer::is_cancelled(&e) => {
            crate::remote::show_status("コピーを中止しました");
            return Ok(());
        }
        Err(e) => return Err(with_path(from, e)),
    };
    let human = crate::util::human_size(size.bytes);
    if cross && size.bytes >= COPY_REMOTE_LIMIT {
        return Err(format!(
            "「{base}」は {human} あります。\n\n\
             このパソコンとリモートの間（別の接続先の間を含む）では、{} MB 以上はコピーできません。",
            COPY_REMOTE_LIMIT >> 20
        ));
    }
    if size.bytes >= COPY_WARN_BYTES {
        let files = if size.files > 1 {
            format!("（{} 個のファイル）", group_digits(size.files))
        } else {
            String::new()
        };
        let text = format!(
            "「{base}」は {human}{files} あります。\n\nコピーしますか？{}",
            if cross {
                "\n（このパソコンとリモートの間で転送するので、時間がかかることがあります）"
            } else {
                ""
            }
        );
        let r = unsafe {
            MessageBoxW(
                Some(hwnd),
                &HSTRING::from(text),
                &HSTRING::from("yyeditor"),
                MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2,
            )
        };
        if r != IDYES {
            return Ok(());
        }
    }
    crate::remote::show_status(&format!("{base} をコピーしています…（Esc で中止）"));
    let too_big = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = too_big.clone();
    let r = crate::remote::wait(&crate::remote::show_status, move |work| {
        yy_remote::transfer::copy(&src, &dst, &mut |st| {
            // 数えたあとでファイルが大きくなった場合も、リモートとの間では上限で止める
            if cross && st.bytes >= COPY_REMOTE_LIMIT {
                flag.store(true, std::sync::atomic::Ordering::Relaxed);
                return false;
            }
            work.report(format!(
                "コピーしています… {} 個のファイル、{}（Esc で中止）",
                group_digits(st.files),
                crate::util::human_size(st.bytes)
            ));
            !work.cancelled()
        })
    });
    if too_big.load(std::sync::atomic::Ordering::Relaxed) {
        crate::remote::show_status("");
        return Err(format!(
            "コピー中に {} MB を超えたため、中止しました（作りかけのコピーは消しました）。",
            COPY_REMOTE_LIMIT >> 20
        ));
    }
    let name = workspace::name_of(&target);
    let msg = match r {
        Ok(st) => {
            let mut m = format!("{name} にコピーしました");
            if st.skipped > 0 {
                m += &format!(
                    "（フォルダを指すリンクなど {} 個は飛ばしました）",
                    group_digits(st.skipped)
                );
            }
            m
        }
        Err(e) if yy_remote::transfer::is_cancelled(&e) => "コピーを中止しました".to_owned(),
        Err(e) => {
            crate::remote::show_status("");
            return Err(with_path(&target, e));
        }
    };
    with_app(|a| {
        a.refresh_folder(to_dir);
        a.status_msg = msg;
        a.update_status();
    });
    Ok(())
}

/// `from` を `to` へ移し（名前を変え）、開いているタブ・ワークスペース・ツリーを合わせる。
fn move_path(from: &Path, to: &Path) -> std::result::Result<(), String> {
    match remote_uri(from) {
        Some(u) => {
            let (f, t) = (
                u.path.clone(),
                remote_uri(to).map(|t| t.path).unwrap_or_default(),
            );
            crate::remote::file_op(&u, "移動しています…", move |s| s.rename(&f, &t))?;
        }
        None => local_move(from, to)?,
    }
    with_app(|a| a.after_path_moved(from, to));
    Ok(())
}

/// 手元のファイル・フォルダを移す（別のドライブへはシェルでコピーしてから消す）。
fn local_move(from: &Path, to: &Path) -> std::result::Result<(), String> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        // ERROR_NOT_SAME_DEVICE
        Err(e) if e.raw_os_error() == Some(17) => {
            let dir = workspace::parent(to).unwrap_or_default();
            shell_op(windows::Win32::UI::Shell::FO_MOVE, from, Some(&dir), 0)
        }
        Err(e) => Err(with_path(from, e)),
    }
}

/// 消す（手元はごみ箱へ、リモートは完全に）。開いているタブは閉じる。
fn delete_item(hwnd: HWND, path: &Path, is_dir: bool) -> std::result::Result<(), String> {
    let docs = with_app(|a| a.documents_within(path)).unwrap_or_default();
    if docs.iter().any(|&(_, modified)| modified) {
        return Err(
            "保存していない変更があるファイルを開いています。保存するか閉じてから削除してください。"
                .into(),
        );
    }
    let remote = remote_uri(path).is_some();
    let name = workspace::name_of(path);
    let mut text = if remote {
        format!(
            "「{name}」を削除しますか？\n\nリモートのファイルはごみ箱に入らず、元に戻せません。"
        )
    } else {
        format!("「{name}」をごみ箱に移動しますか？")
    };
    if is_dir {
        text += "\nフォルダの中身もすべて削除します。";
    }
    if !docs.is_empty() {
        text += &format!("\n\n開いている {} 個のタブを閉じます。", docs.len());
    }
    let r = unsafe {
        MessageBoxW(
            Some(hwnd),
            &HSTRING::from(text),
            &HSTRING::from("yyeditor"),
            MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2,
        )
    };
    if r != IDYES {
        return Ok(());
    }
    // 開いているファイルを先に閉じる（手元のファイルはマップしているので）
    let mut indices: Vec<usize> = docs.iter().map(|&(i, _)| i).collect();
    indices.sort_unstable_by(|a, b| b.cmp(a));
    for i in indices {
        close_tab_at(hwnd, i);
    }
    match remote_uri(path) {
        Some(u) => {
            let p = u.path.clone();
            crate::remote::file_op(&u, "削除しています…", move |s| s.remove(&p, true))?;
        }
        None => {
            use windows::Win32::UI::Shell::{
                FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_WANTNUKEWARNING,
            };
            shell_op(
                FO_DELETE,
                path,
                None,
                (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_WANTNUKEWARNING).0,
            )?;
        }
    }
    with_app(|a| a.after_delete(path));
    Ok(())
}

/// シェルのファイル操作（ごみ箱への削除、別のドライブへの移動）。
fn shell_op(
    func: u32,
    from: &Path,
    to: Option<&Path>,
    flags: u32,
) -> std::result::Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::UI::Shell::{FOF_NOCONFIRMMKDIR, SHFILEOPSTRUCTW, SHFileOperationW};
    // 文字列は 2 つの NUL で終える
    let wide = |p: &Path| -> Vec<u16> { p.as_os_str().encode_wide().chain([0, 0]).collect() };
    let from_w = wide(from);
    let to_w = to.map(wide);
    let frame = with_app(|a| a.frame).unwrap_or_default();
    let mut op = SHFILEOPSTRUCTW {
        hwnd: frame,
        wFunc: func,
        pFrom: windows::core::PCWSTR(from_w.as_ptr()),
        pTo: to_w.as_ref().map_or(windows::core::PCWSTR::null(), |t| {
            windows::core::PCWSTR(t.as_ptr())
        }),
        fFlags: (flags | FOF_NOCONFIRMMKDIR.0) as u16,
        ..Default::default()
    };
    let r = unsafe { SHFileOperationW(&mut op) };
    if r != 0 {
        return Err(with_path(from, format!("操作できませんでした（{r:#x}）")));
    }
    if op.fAnyOperationsAborted.as_bool() {
        return Err(with_path(from, "中止しました"));
    }
    Ok(())
}

impl App {
    /// `dir` を表示しているフォルダの項目を読み直す。
    fn refresh_folder(&mut self, dir: &Path) {
        let items: Vec<HTREEITEM> = self
            .ws
            .nodes
            .iter()
            .filter(|n| n.alive && n.is_dir && yy_config::recent::same_path(&n.path, dir))
            .map(|n| n.item)
            .collect();
        for item in items {
            self.refresh_item(item);
        }
    }

    /// `path`（またはその中）を開いているタブ（番号, 変更があるか）。
    fn documents_within(&self, path: &Path) -> Vec<(usize, bool)> {
        (0..self.tabs.len())
            .filter_map(|i| {
                let doc = if i == self.active_tab {
                    &self.doc
                } else {
                    &self.tabs[i].as_ref()?.doc
                };
                let loc = doc.location()?;
                workspace::is_within(&loc, path).then(|| (i, doc.is_modified() || doc.is_saving()))
            })
            .collect()
    }

    /// `from` を `to` に移した（名前を変えた）あと: 開いているタブ・ワークスペースの起点・切り取り・ツリーを合わせる。
    fn after_path_moved(&mut self, from: &Path, to: &Path) {
        let fix = |d: &mut Document| {
            if let Some(n) = d
                .location()
                .and_then(|loc| workspace::relocated(&loc, from, to))
            {
                d.relocate(&n);
            }
        };
        fix(&mut self.doc);
        for t in self.tabs.iter_mut().flatten() {
            fix(&mut t.doc);
        }
        if let Some(c) = &self.ws.clip
            && workspace::is_within(&c.path, from)
        {
            self.ws.clip = None;
        }
        let mut roots_changed = false;
        for f in &mut self.ws.workspace.folders {
            if let Some(n) = workspace::relocated(f, from, to) {
                *f = n;
                roots_changed = true;
            }
        }
        if roots_changed {
            self.save_workspace();
            self.rebuild_tree();
        } else {
            for dir in [workspace::parent(from), workspace::parent(to)]
                .into_iter()
                .flatten()
            {
                self.refresh_folder(&dir);
            }
        }
        self.status_msg = format!(
            "{} を {} に移しました",
            workspace::name_of(from),
            to.display()
        );
        self.update_title();
        self.update_status();
    }

    /// `path` を消したあと: ワークスペースの起点・切り取り・ツリーを合わせる。
    fn after_delete(&mut self, path: &Path) {
        if self
            .ws
            .clip
            .as_ref()
            .is_some_and(|c| workspace::is_within(&c.path, path))
        {
            self.ws.clip = None;
        }
        let n = self.ws.workspace.folders.len();
        self.ws
            .workspace
            .folders
            .retain(|f| !workspace::is_within(f, path));
        if self.ws.workspace.folders.len() != n {
            self.save_workspace();
            self.rebuild_tree();
        } else if let Some(dir) = workspace::parent(path) {
            self.refresh_folder(&dir);
        }
        self.status_msg = format!("{} を削除しました", workspace::name_of(path));
        self.update_status();
    }

    /// ドラッグ中の項目の番号と、`(x, y)`（フレームのクライアント座標）の下の項目。
    fn drag_target(&self, x: i32, y: i32) -> Option<(usize, HTREEITEM)> {
        let index = self.ws.drag?;
        let mut pt = windows::Win32::Foundation::POINT { x, y };
        unsafe {
            windows::Win32::Graphics::Gdi::MapWindowPoints(
                Some(self.frame),
                Some(self.ws.tree),
                std::slice::from_mut(&mut pt),
            );
        }
        let mut hit = TVHITTESTINFO {
            pt,
            ..Default::default()
        };
        let item = HTREEITEM(unsafe {
            SendMessageW(
                self.ws.tree,
                TVM_HITTEST,
                None,
                Some(LPARAM(&mut hit as *mut _ as isize)),
            )
            .0
        });
        Some((index, item))
    }

    fn set_drop_highlight(&self, item: HTREEITEM) {
        unsafe {
            SendMessageW(
                self.ws.tree,
                TVM_SELECTITEM,
                Some(WPARAM(TVGN_DROPHILITE as usize)),
                Some(LPARAM(item.0)),
            );
        }
    }

    /// 移動先のフォルダ（項目がファイルならそのフォルダ）。
    fn drop_dir(&self, item: HTREEITEM) -> Option<PathBuf> {
        let i = self.node_of(item)?;
        let n = &self.ws.nodes[i];
        if n.is_dir {
            Some(n.path.clone())
        } else {
            workspace::parent(&n.path)
        }
    }
}

/// ドラッグ中のマウスの移動（フレームの WM_MOUSEMOVE）。ドラッグ中でなければ何もしない。
pub(crate) fn drag_move(x: i32, y: i32) {
    with_app(|a| {
        if let Some((_, item)) = a.drag_target(x, y) {
            a.set_drop_highlight(item);
        }
    });
}

/// ドラッグの終わり（フレームの WM_LBUTTONUP / WM_CAPTURECHANGED）。`drop` ならその位置へ移す。
/// ドラッグ中だったら `true`。
pub(crate) fn drag_end(hwnd: HWND, x: i32, y: i32, drop: bool) -> bool {
    let Some(target) = with_app(|a| {
        let (index, item) = a.drag_target(x, y)?;
        a.ws.drag = None;
        a.set_drop_highlight(HTREEITEM::default());
        let from =
            a.ws.nodes
                .get(index)
                .filter(|n| n.alive)
                .map(|n| n.path.clone());
        Some((from, a.drop_dir(item)))
    })
    .flatten() else {
        return false;
    };
    unsafe {
        let _ = ReleaseCapture();
    }
    if let (true, (Some(from), Some(to_dir))) = (drop, target) {
        // 別の場所へはコピー。同じ場所の中は移動（Ctrl を押していればコピー）
        let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
        let op = if ctrl || !workspace::same_place(&from, &to_dir) {
            WsOp::Copy { from, to_dir }
        } else {
            WsOp::Move { from, to_dir }
        };
        post_op(hwnd, op);
    }
    true
}
