//! ソース管理（Git。16 章）。VS Code のソース管理のビューと同じ形で、サイドバーにエクスプローラーと
//! 切り替えて表示する（ワークスペース > ソース管理、Ctrl+Shift+G）。
//!
//! - ワークスペースのフォルダの下（と、フォルダを含む上のフォルダ）の `.git` を探して一覧にし、どれを
//!   表示するかを上の一覧で 1 つ選ぶ（[`yy_git::discover`]。選んだものは覚えて、次も選ぶ）。
//! - ブランチ（上流との差）、コミットのメッセージとコミット（Ctrl+Enter）、変更の一覧（マージの競合・
//!   ステージ済みの変更・変更）。ファイルをダブルクリック（Enter）で差分、右クリックでステージ・
//!   ステージの解除・変更の破棄・ファイルを開く。
//! - 「…」のメニューでプル・プッシュ・フェッチ・同期・すべてステージ・直前のコミットの修正・ブランチ。
//! - git は作業スレッドで動かし（[`WM_APP_GIT_DONE`]）、画面を止めない。

use std::path::{Path, PathBuf};

use windows::Win32::Graphics::Gdi::COLOR_BTNFACE;
use windows::Win32::UI::Controls::*;
use yy_git::{Found, Git, GitError, Group, Status};

use super::*;

pub(crate) const ID_GIT_VIEW: u16 = 1320;
const GIT_CLASS: windows::core::PCWSTR = w!("YYEditorGit");
/// リポジトリを探し終えた（`LPARAM` は `Box<Vec<Found>>`）
const WM_APP_GIT_FOUND: u32 = WM_APP + 1;
/// git の操作が終わった（`LPARAM` は `Box<Done>`）
const WM_APP_GIT_DONE: u32 = WM_APP + 2;
/// 接続先へ接続してから続ける（`LPARAM` は `Box<Connect>`）。接続の問い合わせ（パスワードなど）を
/// 出すので、状態を借りていないところで行う
const WM_APP_GIT_CONNECT: u32 = WM_APP + 3;

const G_COMBO: u16 = 10;
const G_BRANCH: u16 = 11;
const G_REFRESH: u16 = 12;
const G_MORE: u16 = 13;
const G_MSG: u16 = 14;
const G_COMMIT: u16 = 15;
const G_TREE: u16 = 16;

// メニュー
const M_DIFF: u32 = 1;
const M_OPEN: u32 = 2;
const M_STAGE: u32 = 3;
const M_UNSTAGE: u32 = 4;
const M_DISCARD: u32 = 5;
const M_COPY_PATH: u32 = 6;
const M_STAGE_ALL: u32 = 7;
const M_UNSTAGE_ALL: u32 = 8;
const M_DISCARD_ALL: u32 = 9;
const M_PULL: u32 = 20;
const M_PUSH: u32 = 21;
const M_FETCH: u32 = 22;
const M_SYNC: u32 = 23;
const M_AMEND: u32 = 24;
const M_REDISCOVER: u32 = 25;
const M_LOG: u32 = 26;
const M_NEW_BRANCH: u32 = 27;
const M_EXPLORER: u32 = 28;
/// ブランチの切り替え（番号を足す）
const M_BRANCH_BASE: u32 = 1000;

/// サイドバーに出しているもの。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Side {
    Explorer,
    Git,
}

/// ツリーの項目。
#[derive(Clone, Debug)]
enum Item {
    Root(Group),
    File(Group, yy_git::Change),
    Note,
}

/// 作業スレッドで動かす git の操作。
type GitOp = Box<dyn FnOnce(&Git) -> std::result::Result<String, GitError> + Send>;

/// 接続を待っている操作。
struct Pending {
    label: String,
    clear_message: bool,
    op: GitOp,
}

/// 接続してからすること。
enum Then {
    /// リポジトリを探す
    Discover,
    /// 待っている操作（[`ScmPane::pending`]）を動かす
    Pending,
}

/// 接続の頼み。
struct Connect {
    targets: Vec<yy_remote::uri::Target>,
    then: Then,
}

/// `ssh://…` のリポジトリなら、その場所。
fn remote_of(root: &Path) -> Option<yy_remote::RemoteUri> {
    yy_remote::RemoteUri::parse(&root.to_string_lossy())
}

/// リポジトリの git（接続先なら接続済みの接続を使う。なければ `None`）。
fn git_live(root: &Path) -> Option<Git> {
    match remote_of(root) {
        None => Some(Git::new(root)),
        Some(u) => crate::remote::live_transport(&u.target()).map(|t| Git::remote(t, &u.path)),
    }
}

/// リポジトリの git（接続先に接続していなければ接続する。状態を借りていないところで呼ぶ）。
fn git_connect(frame: HWND, root: &Path) -> Option<Git> {
    match remote_of(root) {
        None => Some(Git::new(root)),
        Some(u) => match crate::remote::transport(&u.target(), &crate::remote::show_status) {
            Ok(t) => Some(Git::remote(t, &u.path)),
            Err(e) => {
                error_box(frame, &e);
                None
            }
        },
    }
}

/// リポジトリの中のファイルの場所（手元のパスか `ssh://…`）。
fn file_location(root: &Path, rel: &str) -> PathBuf {
    match remote_of(root) {
        None => root.join(rel),
        Some(u) => PathBuf::from(
            yy_remote::RemoteUri {
                path: yy_proto::join_path(&u.path, rel.as_bytes()),
                ..u
            }
            .to_string(),
        ),
    }
}

/// 作業スレッドの結果。
pub(crate) struct Done {
    root: PathBuf,
    /// 操作の名前（「プッシュ」など。状態の読み直しだけなら空）
    label: String,
    result: std::result::Result<String, GitError>,
    status: std::result::Result<Status, GitError>,
    /// 成功したらメッセージの欄を空にする（コミット）
    clear_message: bool,
}

/// ソース管理のビュー。
pub(crate) struct ScmPane {
    pub hwnd: HWND,
    combo: HWND,
    branch: HWND,
    refresh: HWND,
    more: HWND,
    msg: HWND,
    commit: HWND,
    tree: HWND,
    pub side: Side,
    repos: Vec<Found>,
    /// 表示しているリポジトリ
    current: Option<PathBuf>,
    status: Option<Status>,
    items: Vec<Item>,
    /// 動かしている操作（あれば次の操作は待ってもらう）
    busy: Option<String>,
    /// リポジトリを探した（ワークスペースが変わったら探し直す）
    discovered: bool,
    /// git の出力の記録
    log: String,
    /// 接続を待っている操作
    pending: Option<Pending>,
}

fn last_repo_file() -> Option<PathBuf> {
    yy_config::config_dir().map(|d| d.join("last-git-repo.txt"))
}

fn child(
    parent: HWND,
    instance: HINSTANCE,
    class: windows::core::PCWSTR,
    text: &str,
    style: WINDOW_STYLE,
    id: u16,
) -> Result<HWND> {
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class,
            &HSTRING::from(text),
            WS_CHILD | WS_VISIBLE | style,
            0,
            0,
            0,
            0,
            Some(parent),
            Some(HMENU(id as isize as *mut _)),
            Some(instance),
            None,
        )
    }
}

/// ソース管理のビューを作る（表示はしない）。
pub(crate) fn create_scm(frame: HWND, instance: HINSTANCE) -> Result<ScmPane> {
    unsafe {
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(git_proc),
            hInstance: instance,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: windows::Win32::Graphics::Gdi::GetSysColorBrush(COLOR_BTNFACE),
            lpszClassName: GIT_CLASS,
            ..Default::default()
        };
        // 2 回目（テストなど）は登録済み
        RegisterClassExW(&wc);
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            GIT_CLASS,
            None,
            WS_CHILD | WS_CLIPCHILDREN,
            0,
            0,
            0,
            0,
            Some(frame),
            None,
            Some(instance),
            None,
        )?;
        let combo = child(
            hwnd,
            instance,
            w!("COMBOBOX"),
            "",
            WS_TABSTOP | WS_VSCROLL | WINDOW_STYLE(CBS_DROPDOWNLIST as u32),
            G_COMBO,
        )?;
        let button = |text: &str, id: u16, extra: u32| {
            child(
                hwnd,
                instance,
                w!("BUTTON"),
                text,
                WS_TABSTOP | WINDOW_STYLE(BS_PUSHBUTTON as u32 | extra),
                id,
            )
        };
        let branch = button("", G_BRANCH, BS_LEFT as u32)?;
        let refresh = button("↻", G_REFRESH, 0)?;
        let more = button("…", G_MORE, 0)?;
        let msg = CreateWindowExW(
            WS_EX_CLIENTEDGE,
            w!("EDIT"),
            None,
            WS_CHILD
                | WS_VISIBLE
                | WS_TABSTOP
                | WS_VSCROLL
                | WINDOW_STYLE((ES_MULTILINE | ES_AUTOVSCROLL | ES_WANTRETURN) as u32),
            0,
            0,
            0,
            0,
            Some(hwnd),
            Some(HMENU(G_MSG as isize as *mut _)),
            Some(instance),
            None,
        )?;
        let commit = button("✓ コミット (Ctrl+Enter)", G_COMMIT, 0)?;
        let tree = child(
            hwnd,
            instance,
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
            G_TREE,
        )?;
        let _ = windows::Win32::UI::Controls::SetWindowTheme(tree, w!("Explorer"), None);
        SendMessageW(
            tree,
            TVM_SETEXTENDEDSTYLE,
            Some(WPARAM(TVS_EX_DOUBLEBUFFER as usize)),
            Some(LPARAM(TVS_EX_DOUBLEBUFFER as isize)),
        );
        let pane = ScmPane {
            hwnd,
            combo,
            branch,
            refresh,
            more,
            msg,
            commit,
            tree,
            side: Side::Explorer,
            repos: Vec::new(),
            current: None,
            status: None,
            items: Vec::new(),
            busy: None,
            discovered: false,
            log: String::new(),
            pending: None,
        };
        pane.update_branch_button();
        Ok(pane)
    }
}

impl ScmPane {
    /// 子のウィンドウ（フォントを当てる）。
    pub(crate) fn controls(&self) -> [HWND; 7] {
        [
            self.combo,
            self.branch,
            self.refresh,
            self.more,
            self.msg,
            self.commit,
            self.tree,
        ]
    }

    /// 中の配置（`w`×`h` はビューの大きさ）。
    pub(crate) fn layout(&self, w: i32, h: i32) {
        let dpi = unsafe { GetDpiForWindow(self.hwnd) }.max(96) as i32;
        let px = |v: i32| v * dpi / 96;
        let (m, row) = (px(4), px(26));
        let inner = (w - 2 * m).max(px(40));
        unsafe {
            let mut y = m;
            let _ = MoveWindow(self.combo, m, y, inner, px(300), true);
            y += row + px(2);
            let small = px(28);
            let bw = (inner - 2 * small - px(4)).max(px(20));
            let _ = MoveWindow(self.branch, m, y, bw, row, true);
            let _ = MoveWindow(self.refresh, m + bw + px(2), y, small, row, true);
            let _ = MoveWindow(self.more, m + bw + small + px(4), y, small, row, true);
            y += row + px(4);
            let mh = px(66);
            let _ = MoveWindow(self.msg, m, y, inner, mh, true);
            y += mh + px(4);
            let _ = MoveWindow(self.commit, m, y, inner, row, true);
            y += row + px(4);
            let _ = MoveWindow(self.tree, 0, y, w, (h - y).max(0), true);
        }
    }

    fn set_text(h: HWND, text: &str) {
        unsafe {
            let _ = SetWindowTextW(h, &HSTRING::from(text));
        }
    }

    fn update_branch_button(&self) {
        let text = match (&self.busy, &self.status) {
            (Some(b), _) => format!("… {b}"),
            (None, Some(s)) => format!("⎇ {}", s.branch_label()),
            (None, None) => "⎇".into(),
        };
        Self::set_text(self.branch, &text);
        let enabled = self.current.is_some();
        for h in [self.branch, self.refresh, self.commit, self.msg] {
            unsafe {
                let _ = EnableWindow(h, enabled);
            }
        }
    }

    fn fill_combo(&self) {
        unsafe {
            SendMessageW(self.combo, CB_RESETCONTENT, None, None);
            let mut sel = -1isize;
            for (i, r) in self.repos.iter().enumerate() {
                let label = if r.linked {
                    format!("{}（リンク）", r.label)
                } else {
                    r.label.clone()
                };
                let w = HSTRING::from(label);
                SendMessageW(
                    self.combo,
                    CB_ADDSTRING,
                    None,
                    Some(LPARAM(w.as_ptr() as isize)),
                );
                if Some(&r.root) == self.current.as_ref() {
                    sel = i as isize;
                }
            }
            SendMessageW(self.combo, CB_SETCURSEL, Some(WPARAM(sel as usize)), None);
            let cue = if self.repos.is_empty() {
                HSTRING::from("（リポジトリがありません）")
            } else {
                HSTRING::from(format!(
                    "表示するリポジトリを選んでください（{} 個）",
                    self.repos.len()
                ))
            };
            SendMessageW(
                self.combo,
                CB_SETCUEBANNER,
                None,
                Some(LPARAM(cue.as_ptr() as isize)),
            );
        }
    }

    fn insert(&mut self, parent: HTREEITEM, text: &str, item: Item, bold: bool) -> HTREEITEM {
        let index = self.items.len();
        self.items.push(item);
        let mut t: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let ins = TVINSERTSTRUCTW {
            hParent: parent,
            hInsertAfter: TVI_LAST,
            Anonymous: TVINSERTSTRUCTW_0 {
                itemex: TVITEMEXW {
                    mask: TVIF_TEXT | TVIF_PARAM | TVIF_STATE,
                    pszText: windows::core::PWSTR(t.as_mut_ptr()),
                    lParam: LPARAM(index as isize),
                    state: if bold { TVIS_BOLD.0 } else { 0 },
                    stateMask: TVIS_BOLD.0,
                    ..Default::default()
                },
            },
        };
        HTREEITEM(unsafe {
            SendMessageW(
                self.tree,
                TVM_INSERTITEMW,
                None,
                Some(LPARAM(&ins as *const _ as isize)),
            )
            .0
        })
    }

    /// 一覧を作り直す（選んでいたファイルを選び直す）。
    fn render(&mut self) {
        let keep = self.selected().and_then(|i| match &self.items[i] {
            Item::File(g, c) => Some((*g, c.path.clone())),
            _ => None,
        });
        unsafe {
            SendMessageW(self.tree, WM_SETREDRAW, Some(WPARAM(0)), None);
            SendMessageW(self.tree, TVM_DELETEITEM, None, Some(LPARAM(TVI_ROOT.0)));
        }
        self.items.clear();
        let note = if !self.discovered {
            Some("リポジトリを探しています…".to_string())
        } else if self.repos.is_empty() {
            Some(
                "ワークスペースのフォルダに Git のリポジトリがありません（ワークスペース > フォルダを追加）"
                    .to_string(),
            )
        } else if self.current.is_none() {
            Some(format!(
                "上の一覧から、表示するリポジトリを 1 つ選んでください（{} 個見つかりました）",
                self.repos.len()
            ))
        } else if self.status.is_none() {
            Some("状態を読んでいます…".to_string())
        } else {
            None
        };
        let mut reselect = HTREEITEM::default();
        if let Some(n) = note {
            self.insert(TVI_ROOT, &n, Item::Note, false);
        } else if let Some(st) = self.status.clone() {
            let mut any = false;
            for (g, title) in [
                (Group::Conflict, "マージの競合"),
                (Group::Staged, "ステージ済みの変更"),
                (Group::Unstaged, "変更"),
            ] {
                let list = st.in_group(g);
                if list.is_empty() {
                    continue;
                }
                any = true;
                let root = self.insert(
                    TVI_ROOT,
                    &format!("{title}（{}）", list.len()),
                    Item::Root(g),
                    true,
                );
                for c in list {
                    let (dir, name) = match c.path.rsplit_once('/') {
                        Some((d, n)) => (d, n),
                        None => ("", c.path.as_str()),
                    };
                    let mut label = format!("{}  {name}", c.letter(g));
                    if let Some(o) = &c.orig {
                        label.push_str(&format!("  ← {o}"));
                    }
                    if !dir.is_empty() {
                        label.push_str(&format!("    {dir}"));
                    }
                    let item = self.insert(root, &label, Item::File(g, c.clone()), false);
                    if keep.as_ref() == Some(&(g, c.path.clone())) {
                        reselect = item;
                    }
                }
                unsafe {
                    SendMessageW(
                        self.tree,
                        TVM_EXPAND,
                        Some(WPARAM(TVE_EXPAND.0 as usize)),
                        Some(LPARAM(root.0)),
                    );
                }
            }
            if !any {
                self.insert(TVI_ROOT, "変更はありません", Item::Note, false);
            }
        }
        unsafe {
            if reselect.0 != 0 {
                SendMessageW(
                    self.tree,
                    TVM_SELECTITEM,
                    Some(WPARAM(TVGN_CARET as usize)),
                    Some(LPARAM(reselect.0)),
                );
            }
            SendMessageW(self.tree, WM_SETREDRAW, Some(WPARAM(1)), None);
            let _ = windows::Win32::Graphics::Gdi::InvalidateRect(Some(self.tree), None, true);
        }
        self.update_branch_button();
    }

    /// 選んでいる項目の番号。
    fn selected(&self) -> Option<usize> {
        let item = HTREEITEM(unsafe {
            SendMessageW(
                self.tree,
                TVM_GETNEXTITEM,
                Some(WPARAM(TVGN_CARET as usize)),
                None,
            )
            .0
        });
        self.item_index(item)
    }

    fn item_index(&self, item: HTREEITEM) -> Option<usize> {
        if item.0 == 0 {
            return None;
        }
        let mut it = TVITEMEXW {
            mask: TVIF_PARAM,
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
        (i < self.items.len()).then_some(i)
    }

    fn add_log(&mut self, text: &str) {
        self.log.push_str(text);
        if !text.ends_with('\n') {
            self.log.push('\n');
        }
        // 古い記録は捨てる
        if self.log.len() > 200_000 {
            let cut = self.log.len() - 150_000;
            let cut = (cut..self.log.len())
                .find(|&i| self.log.is_char_boundary(i))
                .unwrap_or(cut);
            self.log.drain(..cut);
        }
    }
}

impl App {
    /// サイドバーにソース管理を出す（出していれば閉じる）。
    pub(crate) fn toggle_git_view(&mut self) {
        if self.ws.visible && self.scm.side == Side::Git {
            self.ws.visible = false;
            self.layout_children();
            self.update_workspace_menu();
            unsafe {
                let _ = SetFocus(Some(self.view));
            }
            return;
        }
        self.scm.side = Side::Git;
        self.ws.visible = true;
        self.layout_children();
        self.update_workspace_menu();
        if self.scm.discovered {
            self.git_refresh();
        } else {
            self.git_discover();
        }
        unsafe {
            let _ = SetFocus(Some(if self.scm.current.is_some() {
                self.scm.tree
            } else {
                self.scm.combo
            }));
        }
    }

    /// ソース管理を表示しているか。
    pub(crate) fn git_visible(&self) -> bool {
        self.ws.visible && self.scm.side == Side::Git
    }

    /// ワークスペースのフォルダが変わった（表示していれば探し直す）。
    pub(crate) fn git_workspace_changed(&mut self) {
        self.scm.discovered = false;
        if self.git_visible() {
            self.git_discover();
        }
    }

    /// ワークスペースのフォルダの下のリポジトリを探す（作業スレッド）。
    fn git_discover(&mut self) {
        self.scm.discovered = false;
        self.scm.render();
        // リモートのフォルダの接続先に、まだ接続していなければ接続してから探す
        let missing = self.git_remote_targets(true);
        if !missing.is_empty() {
            self.git_request_connect(missing, Then::Discover);
            return;
        }
        self.git_discover_now();
    }

    /// ワークスペースのリモートのフォルダの接続先（`missing` なら、接続していないものだけ）。
    fn git_remote_targets(&self, missing: bool) -> Vec<yy_remote::uri::Target> {
        let mut v: Vec<yy_remote::uri::Target> = Vec::new();
        for f in &self.ws.workspace.folders {
            if let Some(u) = remote_of(f) {
                let t = u.target();
                if v.iter().any(|x| x.same(&t)) {
                    continue;
                }
                if missing && crate::remote::live_transport(&t).is_some() {
                    continue;
                }
                v.push(t);
            }
        }
        v
    }

    /// 接続してから続けるよう頼む（状態を借りていないところで接続する）。
    fn git_request_connect(&mut self, targets: Vec<yy_remote::uri::Target>, then: Then) {
        let boxed = Box::into_raw(Box::new(Connect { targets, then }));
        unsafe {
            if PostMessageW(
                Some(self.scm.hwnd),
                WM_APP_GIT_CONNECT,
                WPARAM(0),
                LPARAM(boxed as isize),
            )
            .is_err()
            {
                drop(Box::from_raw(boxed));
            }
        }
    }

    /// 手元のフォルダと、接続しているリモートのフォルダの下を探す（作業スレッド）。
    fn git_discover_now(&mut self) {
        self.scm.discovered = false;
        self.scm.render();
        let roots = self.ws.workspace.folders.clone();
        // 接続先ごとのリモートのフォルダ（接続できなかったものは飛ばす）
        let mut remote: Vec<(Arc<dyn yy_remote::Transport>, Vec<yy_remote::RemoteUri>)> =
            Vec::new();
        let mut skipped = Vec::new();
        for t in self.git_remote_targets(false) {
            let uris: Vec<yy_remote::RemoteUri> = roots
                .iter()
                .filter_map(|f| remote_of(f))
                .filter(|u| u.target().same(&t))
                .collect();
            match crate::remote::live_transport(&t) {
                Some(tr) => remote.push((tr, uris)),
                None => skipped.push(t.to_string()),
            }
        }
        if !skipped.is_empty() {
            self.scm.add_log(&format!(
                "[リポジトリを探す] 接続していないので探しませんでした: {}",
                skipped.join("、")
            ));
        }
        let hwnd = self.scm.hwnd.0 as isize;
        std::thread::spawn(move || {
            let limits = yy_git::Limits::default();
            let mut found = yy_git::discover(&roots, limits);
            for (t, uris) in &remote {
                found.extend(yy_git::discover_remote(t, uris, limits));
            }
            found.sort_by_key(|f| f.label.to_lowercase());
            let boxed = Box::into_raw(Box::new(found));
            unsafe {
                if PostMessageW(
                    Some(HWND(hwnd as *mut _)),
                    WM_APP_GIT_FOUND,
                    WPARAM(0),
                    LPARAM(boxed as isize),
                )
                .is_err()
                {
                    drop(Box::from_raw(boxed));
                }
            }
        });
    }

    fn git_found(&mut self, found: Vec<Found>) {
        self.scm.discovered = true;
        let last = last_repo_file()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .map(|s| PathBuf::from(s.trim()));
        let keep = self
            .scm
            .current
            .clone()
            .filter(|c| found.iter().any(|f| &f.root == c))
            .or_else(|| last.filter(|l| found.iter().any(|f| &f.root == l)))
            .or_else(|| (found.len() == 1).then(|| found[0].root.clone()));
        self.scm.repos = found;
        if keep != self.scm.current {
            self.scm.status = None;
        }
        self.scm.current = keep;
        self.scm.fill_combo();
        self.scm.render();
        if self.scm.current.is_some() {
            self.git_refresh();
        }
    }

    /// 一覧で選んだリポジトリを表示する。
    fn git_select(&mut self, index: usize) {
        let Some(r) = self.scm.repos.get(index) else {
            return;
        };
        let root = r.root.clone();
        if let Some(f) = last_repo_file() {
            let _ = std::fs::write(f, root.to_string_lossy().as_bytes());
        }
        if self.scm.current.as_ref() != Some(&root) {
            self.scm.current = Some(root);
            self.scm.status = None;
            self.scm.busy = None;
            self.scm.render();
        }
        self.git_refresh();
    }

    /// 状態を読み直す（操作の途中なら何もしない。終わったときに読み直す）。接続先のリポジトリで接続が
    /// 切れていれば接続し直す。
    pub(crate) fn git_refresh(&mut self) {
        if self.scm.busy.is_none() && self.scm.current.is_some() {
            self.git_run("", false, |_| Ok(String::new()));
        }
    }

    /// 自動の読み直し（保存・ウィンドウに戻ったとき）。接続先のリポジトリで接続が切れていれば、
    /// 接続し直さない（パスワードを何度も尋ねない）。
    fn git_refresh_quiet(&mut self) {
        let Some(root) = self.scm.current.clone() else {
            return;
        };
        if git_live(&root).is_some() {
            self.git_refresh();
        }
    }

    /// git の操作を作業スレッドで動かし、終わったら状態を読み直す。
    fn git_run(
        &mut self,
        label: &str,
        clear_message: bool,
        op: impl FnOnce(&Git) -> std::result::Result<String, GitError> + Send + 'static,
    ) {
        let Some(root) = self.scm.current.clone() else {
            return;
        };
        if let Some(b) = &self.scm.busy {
            if !label.is_empty() {
                self.status_msg =
                    format!("Git: {b} の途中です。終わってからもう一度行ってください");
                self.update_status();
            }
            return;
        }
        self.scm.busy = Some(if label.is_empty() {
            "状態を読んでいます".into()
        } else {
            format!("{label}を実行しています")
        });
        self.scm.update_branch_button();
        if !label.is_empty() {
            self.status_msg = format!("Git: {label}を実行しています…");
            self.update_status();
        }
        let Some(git) = git_live(&root) else {
            // 接続先に接続してから動かす
            self.scm.busy = None;
            self.scm.pending = Some(Pending {
                label: label.to_string(),
                clear_message,
                op: Box::new(op),
            });
            if let Some(u) = remote_of(&root) {
                self.git_request_connect(vec![u.target()], Then::Pending);
            }
            return;
        };
        let hwnd = self.scm.hwnd.0 as isize;
        let label = label.to_string();
        std::thread::spawn(move || {
            let result = op(&git);
            let status = git.status();
            let done = Box::into_raw(Box::new(Done {
                root,
                label,
                result,
                status,
                clear_message,
            }));
            unsafe {
                if PostMessageW(
                    Some(HWND(hwnd as *mut _)),
                    WM_APP_GIT_DONE,
                    WPARAM(0),
                    LPARAM(done as isize),
                )
                .is_err()
                {
                    drop(Box::from_raw(done));
                }
            }
        });
    }

    /// 接続し終えた（できなかったものもある）。
    fn git_connected(&mut self, then: Then) {
        match then {
            Then::Discover => self.git_discover_now(),
            Then::Pending => {
                let Some(p) = self.scm.pending.take() else {
                    return;
                };
                let live = self.scm.current.as_deref().and_then(git_live).is_some();
                if live {
                    self.git_run(&p.label, p.clear_message, p.op);
                } else {
                    self.scm.render();
                }
            }
        }
    }

    /// 操作が終わった。知らせる誤りを返す（メッセージボックスは借りずに出す）。
    fn git_done(&mut self, done: Done) -> Option<String> {
        if self.scm.current.as_ref() != Some(&done.root) {
            return None;
        }
        self.scm.busy = None;
        let mut error = None;
        match &done.result {
            Ok(text) => {
                if !done.label.is_empty() {
                    self.scm.add_log(&format!("[{}] {}", done.label, text));
                    self.status_msg = if text.trim().is_empty() {
                        format!("Git: {}が終わりました", done.label)
                    } else {
                        format!("Git: {}: {}", done.label, text.lines().next().unwrap_or(""))
                    };
                    self.update_status();
                }
                if done.clear_message {
                    ScmPane::set_text(self.scm.msg, "");
                }
            }
            Err(e) => {
                self.scm.add_log(&format!("[{}] 失敗\n{e}", done.label));
                self.status_msg = format!("Git: {}に失敗しました", done.label);
                self.update_status();
                error = Some(format!("{}に失敗しました。\n\n{e}", done.label));
            }
        }
        match done.status {
            Ok(st) => self.scm.status = Some(st),
            Err(e) => {
                self.scm.add_log(&format!("[状態] 失敗\n{e}"));
                self.scm.status = None;
                if error.is_none() {
                    error = Some(format!("リポジトリの状態を読めません。\n\n{e}"));
                }
            }
        }
        self.scm.render();
        error
    }

    /// 保存のあと・ウィンドウに戻ったときに読み直す（表示していれば）。
    pub(crate) fn git_poke(&mut self) {
        if self.git_visible() {
            self.git_refresh_quiet();
        }
    }

    /// コミットのメッセージの欄にフォーカスがあるか（エディタのショートカットを欄に任せる）。
    pub(crate) fn git_message_has_focus(&self) -> bool {
        self.git_visible() && unsafe { GetFocus() } == self.scm.msg
    }
}

/// メッセージの欄にフォーカスがあるか（メッセージ ループから）。
pub(crate) fn git_message_with_focus() -> bool {
    with_app(|a| a.git_message_has_focus()).unwrap_or(false)
}

/// メッセージ ループで先に処理するキー（メッセージの欄の Ctrl+Enter でコミット）。処理したら `true`。
pub(crate) fn git_pre_translate(msg: &MSG) -> bool {
    if msg.message != WM_KEYDOWN || msg.wParam.0 != VK_RETURN.0 as usize {
        return false;
    }
    if !key_down(VK_CONTROL) {
        return false;
    }
    let Some(pane) = with_app(|a| a.git_message_has_focus().then_some(a.scm.hwnd)).flatten() else {
        return false;
    };
    unsafe {
        let _ = PostMessageW(Some(pane), WM_COMMAND, WPARAM(G_COMMIT as usize), LPARAM(0));
    }
    true
}

/// メッセージの欄の文字列。
fn window_text(h: HWND) -> String {
    unsafe {
        let n = GetWindowTextLengthW(h).max(0) as usize;
        let mut buf = vec![0u16; n + 1];
        let got = GetWindowTextW(h, &mut buf).max(0) as usize;
        String::from_utf16_lossy(&buf[..got])
    }
}

fn ask(owner: HWND, text: &str) -> bool {
    unsafe {
        MessageBoxW(
            Some(owner),
            &HSTRING::from(text),
            w!("yyeditor - ソース管理"),
            MB_YESNO | MB_ICONQUESTION | MB_DEFBUTTON2,
        ) == IDYES
    }
}

/// 版の中身を表示用の文字列（UTF-8）にする（文字コードは推定）。
fn decode(bytes: &[u8]) -> Snapshot {
    let det = yy_encoding::detect(bytes, true);
    let (text, _) = yy_encoding::decode_all(det.encoding, &bytes[det.bom_len..], false);
    Snapshot::from_bytes(text)
}

/// 差分の左右（名前・中身。なければ `None`）。
type Sides = (String, Option<Vec<u8>>, String, Option<Vec<u8>>);

/// ファイルの差分を比較のウィンドウで出す。
fn show_diff(frame: HWND, root: &Path, group: Group, c: &yy_git::Change) {
    let Some(git) = git_connect(frame, root) else {
        return;
    };
    let path = c.path.as_str();
    let orig = c.orig.as_deref().unwrap_or(path);
    let read_work = || git.read_worktree(path).ok().flatten();
    let result: std::result::Result<Sides, GitError> = (|| {
        Ok(match group {
            Group::Staged => (
                format!("{orig}（HEAD）"),
                git.show("HEAD", orig)?,
                format!("{path}（ステージ済み）"),
                git.show("", path)?,
            ),
            Group::Unstaged => {
                let base = if c.untracked() {
                    None
                } else {
                    match git.show("", path)? {
                        Some(b) => Some(b),
                        None => git.show("HEAD", path)?,
                    }
                };
                (
                    format!("{path}（ステージ済み・HEAD）"),
                    base,
                    format!("{path}（作業ツリー）"),
                    read_work(),
                )
            }
            Group::Conflict => (
                format!("{path}（HEAD）"),
                git.show("HEAD", path)?,
                format!("{path}（作業ツリー）"),
                read_work(),
            ),
        })
    })();
    match result {
        Ok((ln, l, rn, r)) => {
            let left = decode(&l.unwrap_or_default());
            let right = decode(&r.unwrap_or_default());
            if let Err(e) = crate::diffview::show(frame, &ln, &left, &rn, &right) {
                error_box(frame, &e);
            }
        }
        Err(e) => error_box(frame, &format!("差分を作れません。\n\n{e}")),
    }
}

/// 変更を捨てる（確かめてから）。
fn discard(frame: HWND, changes: Vec<yy_git::Change>) {
    if changes.is_empty() {
        return;
    }
    let untracked = changes.iter().filter(|c| c.untracked()).count();
    let what = if changes.len() == 1 {
        format!("「{}」", changes[0].path)
    } else {
        format!("{} 個のファイル", changes.len())
    };
    let mut text = format!("{what}の変更を破棄しますか？元に戻せません。");
    if untracked > 0 {
        text.push_str(&format!(
            "\n追跡されていないファイル {untracked} 個は削除します。"
        ));
    }
    if !ask(frame, &text) {
        return;
    }
    let (u, t): (Vec<_>, Vec<_>) = changes.into_iter().partition(|c| c.untracked());
    let tracked: Vec<String> = t.into_iter().map(|c| c.path).collect();
    let untracked: Vec<String> = u.into_iter().map(|c| c.path).collect();
    with_app(|a| {
        a.git_run("変更の破棄", false, move |g| {
            g.discard(&tracked, &untracked).map(|_| String::new())
        })
    });
}

/// コミットする（メッセージ・ステージした変更を確かめてから）。
fn commit(frame: HWND, amend: bool) {
    let Some((msg, has_staged, has_changes, msg_hwnd)) = with_app(|a| {
        let st = a.scm.status.as_ref();
        (
            window_text(a.scm.msg),
            st.is_some_and(Status::has_staged),
            st.is_some_and(|s| !s.changes.is_empty()),
            a.scm.msg,
        )
    }) else {
        return;
    };
    let msg = msg.replace("\r\n", "\n");
    if !amend && msg.trim().is_empty() {
        info_box(
            frame,
            "コミットのメッセージを入力してください（上の欄。Ctrl+Enter でコミット）。",
        );
        unsafe {
            let _ = SetFocus(Some(msg_hwnd));
        }
        return;
    }
    let mut stage_all = false;
    if !has_staged && !amend {
        if !has_changes {
            info_box(frame, "コミットする変更がありません。");
            return;
        }
        if !ask(
            frame,
            "ステージした変更がありません。すべての変更をステージしてコミットしますか？",
        ) {
            return;
        }
        stage_all = true;
    }
    if amend
        && !ask(
            frame,
            "直前のコミットを修正しますか？（ステージした変更を加えます。メッセージが空なら直前のメッセージのまま）\n\
             プッシュ済みのコミットを修正すると、プッシュに --force が必要になります。",
        )
    {
        return;
    }
    let label = if amend {
        "直前のコミットの修正"
    } else {
        "コミット"
    };
    with_app(|a| {
        a.git_run(label, true, move |g| {
            if stage_all {
                g.stage_all()?;
            }
            let msg = if amend && msg.trim().is_empty() {
                g.last_message()?
            } else {
                msg
            };
            g.commit(&msg, amend)
        })
    });
}

/// ブランチのメニュー（切り替え・新しいブランチ）。
fn branch_menu(frame: HWND, anchor: HWND) {
    let Some(root) = with_app(|a| a.scm.current.clone()).flatten() else {
        return;
    };
    let Some(git) = git_connect(frame, &root) else {
        return;
    };
    let branches = match git.branches() {
        Ok(b) => b,
        Err(e) => {
            error_box(frame, &format!("ブランチの一覧を読めません。\n\n{e}"));
            return;
        }
    };
    let cmd = unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        let _ = AppendMenuW(
            menu,
            MF_STRING,
            M_NEW_BRANCH as usize,
            w!("新しいブランチを作成して切り替え(&N)..."),
        );
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        let remote_menu = CreatePopupMenu().ok();
        let mut n_remote = 0;
        for (i, b) in branches.iter().enumerate() {
            let id = M_BRANCH_BASE as usize + i;
            if b.remote {
                if let Some(rm) = remote_menu
                    && n_remote < 200
                {
                    let _ = AppendMenuW(rm, MF_STRING, id, &HSTRING::from(b.name.as_str()));
                    n_remote += 1;
                }
                continue;
            }
            let mut text = b.name.clone();
            if !b.upstream.is_empty() {
                text.push_str(&format!("\t{}", b.upstream));
            }
            let flags = if b.current {
                MF_STRING | MF_CHECKED
            } else {
                MF_STRING
            };
            let _ = AppendMenuW(menu, flags, id, &HSTRING::from(text));
        }
        if let Some(rm) = remote_menu {
            if n_remote > 0 {
                let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
                let _ = AppendMenuW(menu, MF_POPUP, rm.0 as usize, w!("リモートのブランチ(&R)"));
            } else {
                let _ = DestroyMenu(rm);
            }
        }
        let mut rc = RECT::default();
        let _ = GetWindowRect(anchor, &mut rc);
        let cmd = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_NONOTIFY | TPM_LEFTALIGN | TPM_TOPALIGN,
            rc.left,
            rc.bottom,
            None,
            frame,
            None,
        );
        let _ = DestroyMenu(menu);
        cmd.0 as u32
    };
    if cmd == M_NEW_BRANCH {
        let Some(name) = crate::goto::prompt_text(
            frame,
            "新しいブランチ",
            "ブランチの名前（今のコミットから作って切り替えます）:",
            "",
        ) else {
            return;
        };
        let name = name.trim().to_string();
        if name.is_empty() {
            return;
        }
        with_app(|a| {
            a.git_run("ブランチの作成", false, move |g| {
                g.create_branch(&name)
                    .map(|_| format!("ブランチ {name} を作りました"))
            })
        });
    } else if cmd >= M_BRANCH_BASE
        && let Some(b) = branches.get((cmd - M_BRANCH_BASE) as usize).cloned()
    {
        if b.current {
            return;
        }
        with_app(|a| {
            a.git_run("ブランチの切り替え", false, move |g| {
                g.switch(&b).map(|_| format!("{} に切り替えました", b.name))
            })
        });
    }
}

/// 「…」のメニュー。
fn more_menu(frame: HWND, anchor: HWND) {
    let Some((has_repo, repos)) = with_app(|a| (a.scm.current.is_some(), a.scm.repos.len())) else {
        return;
    };
    let cmd = unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        let add = |id: u32, text: &str, enabled: bool| {
            let flags = if enabled {
                MF_STRING
            } else {
                MF_STRING | MF_GRAYED
            };
            let _ = AppendMenuW(menu, flags, id as usize, &HSTRING::from(text));
        };
        let sep = || {
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        };
        add(M_PULL, "プル(&L)", has_repo);
        add(M_PUSH, "プッシュ(&P)", has_repo);
        add(M_SYNC, "同期（プルしてプッシュ）(&Y)", has_repo);
        add(M_FETCH, "フェッチ(&F)", has_repo);
        sep();
        add(M_STAGE_ALL, "すべての変更をステージ(&A)", has_repo);
        add(M_UNSTAGE_ALL, "すべてのステージを解除(&U)", has_repo);
        add(M_DISCARD_ALL, "すべての変更を破棄(&D)...", has_repo);
        sep();
        add(M_AMEND, "直前のコミットを修正(&M)...", has_repo);
        sep();
        add(
            M_REDISCOVER,
            &format!("リポジトリを探し直す(&R)（今は {repos} 個）"),
            true,
        );
        add(M_LOG, "Git の出力を表示(&O)", true);
        add(M_EXPLORER, "エクスプローラーに戻る(&E)\tCtrl+Shift+E", true);
        let mut rc = RECT::default();
        let _ = GetWindowRect(anchor, &mut rc);
        let cmd = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTALIGN | TPM_TOPALIGN,
            rc.right,
            rc.bottom,
            None,
            frame,
            None,
        );
        let _ = DestroyMenu(menu);
        cmd.0 as u32
    };
    run_menu(frame, cmd, None);
}

/// メニューの項目を行う（`target` は右クリックした項目）。
fn run_menu(frame: HWND, cmd: u32, target: Option<Item>) {
    match cmd {
        M_PULL => {
            with_app(|a| a.git_run("プル", false, |g| g.pull().map(|o| summary(&o))));
        }
        M_PUSH => {
            with_app(|a| {
                a.git_run("プッシュ", false, |g| {
                    let st = g.status()?;
                    g.push(&st).map(|o| summary(&o))
                })
            });
        }
        M_SYNC => {
            with_app(|a| {
                a.git_run("同期", false, |g| {
                    let pulled = g.pull()?;
                    let st = g.status()?;
                    let pushed = g.push(&st)?;
                    Ok(format!("{}{}", summary(&pulled), summary(&pushed)))
                })
            });
        }
        M_FETCH => {
            with_app(|a| a.git_run("フェッチ", false, |g| g.fetch().map(|o| summary(&o))));
        }
        M_STAGE_ALL => {
            with_app(|a| {
                a.git_run("すべてのステージ", false, |g| {
                    g.stage_all().map(|_| String::new())
                })
            });
        }
        M_UNSTAGE_ALL => {
            with_app(|a| {
                a.git_run("ステージの解除", false, |g| {
                    g.unstage_all().map(|_| String::new())
                })
            });
        }
        M_DISCARD_ALL => {
            let changes: Vec<yy_git::Change> = with_app(|a| {
                a.scm
                    .status
                    .as_ref()
                    .map(|s| s.in_group(Group::Unstaged).into_iter().cloned().collect())
            })
            .flatten()
            .unwrap_or_default();
            discard(frame, changes);
        }
        M_AMEND => commit(frame, true),
        M_REDISCOVER => {
            with_app(|a| a.git_discover());
        }
        M_LOG => {
            let log = with_app(|a| a.scm.log.clone()).unwrap_or_default();
            let tail: String = {
                let chars: Vec<char> = log.chars().collect();
                chars[chars.len().saturating_sub(3000)..].iter().collect()
            };
            info_box(
                frame,
                &if tail.trim().is_empty() {
                    "まだ Git の出力はありません。".to_string()
                } else {
                    format!("Git の出力（最近のもの）:\n\n{tail}")
                },
            );
        }
        M_EXPLORER => {
            with_app(|a| a.toggle_sidebar());
        }
        M_DIFF | M_OPEN | M_STAGE | M_UNSTAGE | M_DISCARD | M_COPY_PATH => {
            let Some(Item::File(group, c)) = target else {
                return;
            };
            let Some(root) = with_app(|a| a.scm.current.clone()).flatten() else {
                return;
            };
            match cmd {
                M_DIFF => show_diff(frame, &root, group, &c),
                M_OPEN => open_path(frame, file_location(&root, &c.path), None, false),
                M_STAGE => {
                    let p = vec![c.path.clone()];
                    with_app(|a| {
                        a.git_run("ステージ", false, move |g| {
                            g.stage(&p).map(|_| String::new())
                        })
                    });
                }
                M_UNSTAGE => {
                    let mut p = vec![c.path.clone()];
                    // 名前の変更は元の名前も戻す
                    if let Some(o) = &c.orig {
                        p.push(o.clone());
                    }
                    with_app(|a| {
                        a.git_run("ステージの解除", false, move |g| {
                            g.unstage(&p).map(|_| String::new())
                        })
                    });
                }
                M_DISCARD => discard(frame, vec![c]),
                M_COPY_PATH => {
                    // 接続先のものは接続先のパス（/home/…）
                    let text = match remote_of(&root) {
                        Some(u) => String::from_utf8_lossy(&yy_proto::join_path(
                            &u.path,
                            c.path.as_bytes(),
                        ))
                        .into_owned(),
                        None => root.join(&c.path).to_string_lossy().into_owned(),
                    };
                    crate::clipboard::set_text(frame, &text, false);
                }
                _ => {}
            }
        }
        _ => {}
    }
}

/// git の出力の短い要約（なければ空）。
fn summary(o: &yy_git::Output) -> String {
    let text = format!("{}\n{}", o.text(), o.stderr);
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    lines.last().map(|l| l.to_string()).unwrap_or_default()
}

/// 変更の一覧の右クリック。
fn tree_menu(frame: HWND) {
    let Some(tree) = with_app(|a| a.scm.tree) else {
        return;
    };
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
    if item.0 == 0 {
        return;
    }
    unsafe {
        SendMessageW(
            tree,
            TVM_SELECTITEM,
            Some(WPARAM(TVGN_CARET as usize)),
            Some(LPARAM(item.0)),
        );
    }
    let Some(target) =
        with_app(|a| a.scm.item_index(item).map(|i| a.scm.items[i].clone())).flatten()
    else {
        return;
    };
    let cmd = unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        let add = |id: u32, text: &str| {
            let _ = AppendMenuW(menu, MF_STRING, id as usize, &HSTRING::from(text));
        };
        let sep = || {
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        };
        match &target {
            Item::File(g, _) => {
                add(M_DIFF, "差分を表示(&D)\tEnter");
                add(M_OPEN, "ファイルを開く(&O)");
                sep();
                match g {
                    Group::Staged => add(M_UNSTAGE, "ステージを解除(&U)"),
                    Group::Unstaged => {
                        add(M_STAGE, "ステージ(&S)");
                        add(M_DISCARD, "変更を破棄(&R)...");
                    }
                    Group::Conflict => add(M_STAGE, "解決済みにする（ステージ）(&S)"),
                }
                sep();
                add(M_COPY_PATH, "パスをコピー(&C)");
            }
            Item::Root(Group::Staged) => add(M_UNSTAGE_ALL, "すべてのステージを解除(&U)"),
            Item::Root(_) => {
                add(M_STAGE_ALL, "すべての変更をステージ(&S)");
                add(M_DISCARD_ALL, "すべての変更を破棄(&R)...");
            }
            Item::Note => {
                let _ = DestroyMenu(menu);
                return;
            }
        }
        let cmd = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
            screen.x,
            screen.y,
            None,
            frame,
            None,
        );
        let _ = DestroyMenu(menu);
        cmd.0 as u32
    };
    run_menu(frame, cmd, Some(target));
}

/// 選んでいるファイルの差分を出す（ダブルクリック・Enter）。
fn diff_selected(frame: HWND) {
    let target = with_app(|a| {
        let i = a.scm.selected()?;
        let root = a.scm.current.clone()?;
        match &a.scm.items[i] {
            Item::File(g, c) => Some((root, *g, c.clone())),
            _ => None,
        }
    })
    .flatten();
    if let Some((root, g, c)) = target {
        show_diff(frame, &root, g, &c);
    }
}

extern "system" fn git_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let frame = || unsafe { GetParent(hwnd).unwrap_or_default() };
    match msg {
        WM_COMMAND => {
            let id = loword(wparam.0) as u16;
            let code = hiword(wparam.0);
            match id {
                G_COMBO if code == CBN_SELCHANGE => {
                    let i = unsafe {
                        SendMessageW(HWND(lparam.0 as *mut _), CB_GETCURSEL, None, None).0
                    };
                    if i >= 0 {
                        with_app(|a| a.git_select(i as usize));
                    }
                }
                G_REFRESH => {
                    with_app(|a| a.git_refresh());
                }
                G_COMMIT => commit(frame(), false),
                G_BRANCH => {
                    if let Some(b) = with_app(|a| a.scm.branch) {
                        branch_menu(frame(), b);
                    }
                }
                G_MORE => {
                    if let Some(m) = with_app(|a| a.scm.more) {
                        more_menu(frame(), m);
                    }
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_NOTIFY => {
            let hdr = unsafe { &*(lparam.0 as *const NMHDR) };
            if hdr.idFrom != G_TREE as usize {
                return LRESULT(0);
            }
            match hdr.code {
                NM_DBLCLK => {
                    diff_selected(frame());
                    LRESULT(1)
                }
                NM_RCLICK => {
                    tree_menu(frame());
                    LRESULT(1)
                }
                TVN_KEYDOWN => {
                    let nm = unsafe { &*(lparam.0 as *const NMTVKEYDOWN) };
                    if nm.wVKey == VK_RETURN.0 {
                        diff_selected(frame());
                        return LRESULT(1);
                    }
                    if nm.wVKey == VK_APPS.0 {
                        tree_menu(frame());
                        return LRESULT(1);
                    }
                    LRESULT(0)
                }
                _ => LRESULT(0),
            }
        }
        WM_APP_GIT_FOUND => {
            let found = unsafe { Box::from_raw(lparam.0 as *mut Vec<Found>) };
            with_app(|a| a.git_found(*found));
            LRESULT(0)
        }
        WM_APP_GIT_CONNECT => {
            let req = unsafe { Box::from_raw(lparam.0 as *mut Connect) };
            for t in &req.targets {
                if crate::remote::live_transport(t).is_some() {
                    continue;
                }
                if let Err(e) = crate::remote::transport(t, &crate::remote::show_status) {
                    error_box(frame(), &e);
                }
            }
            with_app(|a| a.git_connected(req.then));
            LRESULT(0)
        }
        WM_APP_GIT_DONE => {
            let done = unsafe { Box::from_raw(lparam.0 as *mut Done) };
            if let Some(Some(e)) = with_app(|a| a.git_done(*done)) {
                error_box(frame(), &e);
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}
