//! ターミナルのワークスペース（左のサイドバー）。
//!
//! エディタと同じ `*.yyworkspace`（[`yy_config::workspace`]）を使い、手元のフォルダと
//! SSH 接続先のフォルダ（`ssh://…`）を並べる。フォルダをダブルクリックするとそこで
//! ターミナルを開き、ファイルはエディタ（yyeditor）で開く。

use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, WPARAM};
use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL};
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::Shell::{
    SHFILEINFOW, SHGFI_OPENICON, SHGFI_SMALLICON, SHGFI_SYSICONINDEX, SHGFI_USEFILEATTRIBUTES,
    SHGetFileInfoW,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PWSTR, Result, w};
use yy_config::workspace::{self, Workspace};
use yy_remote::RemoteUri;

use crate::util::Context;

/// サイドバーのツリー ビューの ID
pub(crate) const ID_TREE: u16 = 2100;

/// ツリーの 1 項目。
pub(crate) struct Node {
    /// 束ねたフォルダ（`a/b/c`。VS Code と同じ）では、いちばん奥のフォルダ
    pub path: PathBuf,
    /// 表示している名前
    label: String,
    pub is_dir: bool,
    pub root: bool,
    loaded: bool,
    item: HTREEITEM,
}

impl Node {
    pub(crate) fn remote(&self) -> Option<RemoteUri> {
        remote_uri(&self.path)
    }
}

pub(crate) fn remote_uri(path: &Path) -> Option<RemoteUri> {
    RemoteUri::parse(path.to_str()?)
}

/// サイドバー。
pub(crate) struct Sidebar {
    pub tree: HWND,
    pub visible: bool,
    pub workspace: Workspace,
    /// ワークスペースのファイル（名前を付けていなければ `terminal-untitled.yyworkspace`）
    pub file: Option<PathBuf>,
    nodes: Vec<Node>,
    /// ツリーを作り直した回数（読み込みの待ち合わせに使う）
    pub generation: usize,
    /// システムのアイコンの番号（フォルダ・開いたフォルダ・ファイル）
    icons: (i32, i32, i32),
}

fn system_icon(
    name: &str,
    attr: windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES,
    open: bool,
) -> i32 {
    let mut info = SHFILEINFOW::default();
    let mut flags = SHGFI_SYSICONINDEX | SHGFI_SMALLICON | SHGFI_USEFILEATTRIBUTES;
    if open {
        flags |= SHGFI_OPENICON;
    }
    let name: Vec<u16> = name.encode_utf16().chain([0]).collect();
    unsafe {
        SHGetFileInfoW(
            windows::core::PCWSTR(name.as_ptr()),
            attr,
            Some(&mut info),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            flags,
        );
    }
    info.iIcon
}

impl Sidebar {
    pub(crate) fn create(frame: HWND, instance: HINSTANCE) -> Result<Sidebar> {
        let tree = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                WC_TREEVIEWW,
                None,
                WS_CHILD
                    | WS_TABSTOP
                    | WINDOW_STYLE(
                        TVS_HASBUTTONS | TVS_LINESATROOT | TVS_SHOWSELALWAYS | TVS_FULLROWSELECT,
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
        let mut info = SHFILEINFOW::default();
        unsafe {
            let _ = SetWindowTheme(tree, w!("Explorer"), None);
            SendMessageW(
                tree,
                TVM_SETEXTENDEDSTYLE,
                Some(WPARAM(TVS_EX_DOUBLEBUFFER as usize)),
                Some(LPARAM(TVS_EX_DOUBLEBUFFER as isize)),
            );
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
        let icons = (
            system_icon("folder", FILE_ATTRIBUTE_DIRECTORY, false),
            system_icon("folder", FILE_ATTRIBUTE_DIRECTORY, true),
            system_icon("file", FILE_ATTRIBUTE_NORMAL, false),
        );
        // ターミナルが最後に使ったワークスペース、なければエディタのもの
        let file = workspace::last_used_in(workspace::TERMINAL_LAST_FILE)
            .or_else(workspace::last_used)
            .filter(|f| f.is_file())
            .or_else(|| workspace::config_file(workspace::TERMINAL_UNTITLED_FILE));
        let ws = file
            .as_deref()
            .and_then(|f| Workspace::load(f).ok())
            .unwrap_or_default();
        let mut s = Sidebar {
            tree,
            visible: !ws.folders.is_empty(),
            workspace: ws,
            file,
            nodes: Vec::new(),
            generation: 0,
            icons,
        };
        s.rebuild();
        Ok(s)
    }

    fn insert(&mut self, parent: HTREEITEM, mut node: Node, label: &str) {
        node.label = label.to_owned();
        let (image, selected) = if node.is_dir {
            (self.icons.0, self.icons.1)
        } else {
            (self.icons.2, self.icons.2)
        };
        let index = self.nodes.len();
        let children = i32::from(node.is_dir);
        self.nodes.push(node);
        let mut text: Vec<u16> = label.encode_utf16().chain([0]).collect();
        let ins = TVINSERTSTRUCTW {
            hParent: parent,
            hInsertAfter: TVI_LAST,
            Anonymous: TVINSERTSTRUCTW_0 {
                itemex: TVITEMEXW {
                    mask: TVIF_TEXT | TVIF_PARAM | TVIF_CHILDREN | TVIF_IMAGE | TVIF_SELECTEDIMAGE,
                    pszText: PWSTR(text.as_mut_ptr()),
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
                    self.tree,
                    TVM_INSERTITEMW,
                    None,
                    Some(LPARAM(&ins as *const _ as isize)),
                )
                .0,
            )
        };
        self.nodes[index].item = item;
    }

    /// ツリーを作り直す（起点のフォルダだけを並べる）。
    pub(crate) fn rebuild(&mut self) {
        unsafe {
            SendMessageW(self.tree, WM_SETREDRAW, Some(WPARAM(0)), None);
            SendMessageW(self.tree, TVM_DELETEITEM, None, Some(LPARAM(TVI_ROOT.0)));
        }
        self.nodes.clear();
        self.generation += 1;
        for f in self.workspace.folders.clone() {
            let name = workspace::name_of(&f);
            let label = match remote_uri(&f) {
                Some(u) => format!("{name} [{}]", u.target()),
                None => name,
            };
            self.insert(
                TVI_ROOT,
                Node {
                    path: f,
                    label: String::new(),
                    is_dir: true,
                    root: true,
                    loaded: false,
                    item: HTREEITEM::default(),
                },
                &label,
            );
        }
        unsafe {
            SendMessageW(self.tree, WM_SETREDRAW, Some(WPARAM(1)), None);
            let _ = windows::Win32::Graphics::Gdi::InvalidateRect(Some(self.tree), None, true);
        }
    }

    /// 項目の番号。
    pub(crate) fn node_of(&self, item: HTREEITEM) -> Option<usize> {
        let mut tv = TVITEMEXW {
            mask: TVIF_PARAM | TVIF_HANDLE,
            hItem: item,
            ..Default::default()
        };
        let ok = unsafe {
            SendMessageW(
                self.tree,
                TVM_GETITEMW,
                None,
                Some(LPARAM(&mut tv as *mut _ as isize)),
            )
            .0
        };
        (ok != 0 && (tv.lParam.0 as usize) < self.nodes.len()).then_some(tv.lParam.0 as usize)
    }

    pub(crate) fn selected(&self) -> HTREEITEM {
        HTREEITEM(unsafe {
            SendMessageW(
                self.tree,
                TVM_GETNEXTITEM,
                Some(WPARAM(TVGN_CARET as usize)),
                None,
            )
            .0
        })
    }

    /// 選択している項目の `(番号, パス, フォルダか, 起点か)`。
    pub(crate) fn selected_node(&self) -> Option<(usize, PathBuf, bool, bool)> {
        let i = self.node_of(self.selected())?;
        let n = &self.nodes[i];
        Some((i, n.path.clone(), n.is_dir, n.root))
    }

    /// フォルダの中身を読む（まだなら）。リモートのフォルダは後で読む必要があるので、
    /// そのときはその項目の番号を返す。
    pub(crate) fn expanding(&mut self, item: HTREEITEM) -> Option<usize> {
        let i = self.node_of(item)?;
        if self.nodes[i].loaded || !self.nodes[i].is_dir {
            return None;
        }
        if self.nodes[i].remote().is_some() {
            return Some(i);
        }
        self.nodes[i].loaded = true;
        let dir = self.nodes[i].path.clone();
        let mut entries = workspace::list_dir(&dir)
            .map(|(e, _)| e)
            .unwrap_or_default();
        // 中身がフォルダ 1 つだけのフォルダは束ねる（VS Code と同じ）
        workspace::compact_local(&mut entries);
        self.add_children(i, entries);
        None
    }

    /// 読んだ中身を `index` の項目の下に並べる。
    pub(crate) fn add_children(&mut self, index: usize, entries: Vec<workspace::Entry>) {
        let item = self.nodes[index].item;
        self.nodes[index].loaded = true;
        if entries.is_empty() {
            let tv = TVITEMEXW {
                mask: TVIF_CHILDREN | TVIF_HANDLE,
                hItem: item,
                cChildren: TVITEMEXW_CHILDREN(0),
                ..Default::default()
            };
            unsafe {
                SendMessageW(
                    self.tree,
                    TVM_SETITEMW,
                    None,
                    Some(LPARAM(&tv as *const _ as isize)),
                );
            }
            return;
        }
        for e in entries {
            self.insert(
                item,
                Node {
                    path: e.path,
                    label: String::new(),
                    is_dir: e.is_dir,
                    root: false,
                    loaded: false,
                    item: HTREEITEM::default(),
                },
                &e.name,
            );
        }
    }

    /// まだ読んでいないリモートのフォルダ `index` の中身がフォルダ `only` 1 つだけだったとき、
    /// 束ねて（`a/b`）、その中のフォルダを読むよう返す（起点のフォルダは束ねない）。
    pub(crate) fn merge_single(
        &mut self,
        generation: usize,
        index: usize,
        only: &workspace::Entry,
    ) -> Option<RemoteUri> {
        self.pending_remote(generation, index)?;
        let n = &mut self.nodes[index];
        if n.root || !only.is_dir {
            return None;
        }
        let next = remote_uri(&only.path)?;
        n.path = only.path.clone();
        n.label = format!("{}/{}", n.label, only.name);
        let mut text: Vec<u16> = n.label.encode_utf16().chain([0]).collect();
        let tv = TVITEMEXW {
            mask: TVIF_TEXT | TVIF_HANDLE,
            hItem: n.item,
            pszText: PWSTR(text.as_mut_ptr()),
            ..Default::default()
        };
        unsafe {
            SendMessageW(
                self.tree,
                TVM_SETITEMW,
                None,
                Some(LPARAM(&tv as *const _ as isize)),
            );
        }
        Some(next)
    }

    /// 項目を開く。
    pub(crate) fn expand(&self, index: usize) {
        if let Some(n) = self.nodes.get(index) {
            unsafe {
                SendMessageW(
                    self.tree,
                    TVM_EXPAND,
                    Some(WPARAM(TVE_EXPAND.0 as usize)),
                    Some(LPARAM(n.item.0)),
                );
            }
        }
    }

    /// まだ読んでいないリモートのフォルダか（待っている間に作り直されていないか確かめる）。
    pub(crate) fn pending_remote(&self, generation: usize, index: usize) -> Option<RemoteUri> {
        if generation != self.generation {
            return None;
        }
        let n = self.nodes.get(index)?;
        if n.loaded {
            return None;
        }
        n.remote()
    }

    /// ワークスペースを保存し、ターミナルが最後に使ったものとして記録する。
    pub(crate) fn save(&mut self) -> std::result::Result<(), String> {
        let file = match &self.file {
            Some(f) => f.clone(),
            None => match workspace::config_file(workspace::TERMINAL_UNTITLED_FILE) {
                Some(f) => f,
                None => return Ok(()),
            },
        };
        self.file = Some(file.clone());
        self.workspace
            .save(&file)
            .map_err(|e| format!("ワークスペースを保存できません: {e}"))?;
        let _ = workspace::set_last_used_in(workspace::TERMINAL_LAST_FILE, &file);
        Ok(())
    }

    /// 起点のフォルダを加える（既にあれば `false`）。
    pub(crate) fn add_folder(&mut self, dir: &Path) -> bool {
        if !self.workspace.add(dir) {
            return false;
        }
        self.rebuild();
        self.visible = true;
        true
    }

    pub(crate) fn remove_folder(&mut self, dir: &Path) {
        if self.workspace.remove(dir) {
            self.rebuild();
        }
    }

    /// 別のワークスペースのファイルに切り替える。
    pub(crate) fn switch_to(&mut self, file: Option<PathBuf>, ws: Workspace) {
        self.file = file;
        self.workspace = ws;
        self.rebuild();
    }

    /// ワークスペースの名前（ウィンドウのタイトル用。名前を付けていなければ `None`）。
    pub(crate) fn title(&self) -> Option<String> {
        let f = self.file.as_ref()?;
        if workspace::config_file(workspace::TERMINAL_UNTITLED_FILE).as_ref() == Some(f) {
            return None;
        }
        f.file_stem().map(|s| s.to_string_lossy().into_owned())
    }
}
