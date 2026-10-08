//! yysheet のワークスペース（左のサイドバー。15 章 12.2.1）。
//!
//! エディタ・ターミナルと同じ `*.yyworkspace` に、手元のフォルダと SSH 接続先のフォルダ（`ssh://…`）を
//! 並べる。ファイルをダブルクリック（Enter）すると開く（接続先のファイルは取り寄せて開き、保存すると
//! 送り返す。[`super::remotefile`]）。右クリックで、固定長ファイルとして開く・シートとして追加するなど。

use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::Input::KeyboardAndMouse::{SetFocus, VK_DELETE, VK_RETURN};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, Result};
use yy_config::workspace::{self, Workspace};

use super::remotefile::{self, How, remote_uri};
use super::{set_status, with};
use crate::util::{error_box, info_box};

/// リモートのフォルダを読む（ツリーを開いたあと。`wParam` = 作り直した回数、`lParam` = 項目の番号）
pub(super) const WM_APP_SHEET_REMOTE_DIR: u32 = WM_APP + 42;

pub(super) const ID_SIDEBAR: u16 = 200;
pub(super) const ID_WS_NEW: u16 = 201;
pub(super) const ID_WS_OPEN: u16 = 202;
pub(super) const ID_WS_SAVE_AS: u16 = 203;
pub(super) const ID_WS_ADD: u16 = 204;
pub(super) const ID_WS_ADD_REMOTE: u16 = 205;
pub(super) const ID_WS_USE_AGENT: u16 = 206;
pub(super) const ID_OPEN_REMOTE: u16 = 207;

/// サイドバーの幅（DIP）
pub(super) const SIDEBAR_WIDTH: i32 = 240;

// 右クリックのメニュー
const CM_OPEN: u32 = 1;
const CM_OPEN_MULTI: u32 = 2;
const CM_ADD_FIXED: u32 = 3;
const CM_ADD_MULTI: u32 = 4;
const CM_DOWNLOAD: u32 = 5;
const CM_COPY_PATH: u32 = 6;
const CM_REFRESH: u32 = 7;
const CM_REMOVE: u32 = 8;
const CM_ADD: u32 = 9;
const CM_ADD_REMOTE: u32 = 10;
const CM_OPEN_DELIMITED: u32 = 11;

/// メニュー バーの「ワークスペース」と、表示メニューの切り替え。
pub(super) fn add_menus(bar: HMENU, view: HMENU) -> Result<()> {
    unsafe {
        let add = |m: HMENU, id: u16, text: &str| {
            let _ = AppendMenuW(m, MF_STRING, id as usize, &HSTRING::from(text));
        };
        AppendMenuW(view, MF_SEPARATOR, 0, None)?;
        add(view, ID_SIDEBAR, "ワークスペース(&W)\tCtrl+Shift+E");
        let ws = CreatePopupMenu()?;
        add(ws, ID_WS_NEW, "新しいワークスペース(&N)");
        add(ws, ID_WS_OPEN, "ワークスペースを開く(&O)...");
        add(ws, ID_WS_SAVE_AS, "名前を付けて保存(&A)...");
        AppendMenuW(ws, MF_SEPARATOR, 0, None)?;
        add(ws, ID_WS_ADD, "フォルダを追加(&F)...");
        add(ws, ID_WS_ADD_REMOTE, "リモートのフォルダを追加(&R)...");
        AppendMenuW(ws, MF_SEPARATOR, 0, None)?;
        add(
            ws,
            ID_WS_USE_AGENT,
            "リモートの読み書きに接続先のエージェントを使う(&G)",
        );
        // 「ヘルプ」の前に置く
        let n = GetMenuItemCount(Some(bar));
        InsertMenuW(
            bar,
            (n - 1).max(0) as u32,
            MF_BYPOSITION | MF_POPUP,
            ws.0 as usize,
            &HSTRING::from("ワークスペース(&W)"),
        )?;
    }
    Ok(())
}

/// エージェントを使うかの印をメニューに付ける。
pub(super) fn check_use_agent(frame: HWND) {
    unsafe {
        let on = crate::remote::use_agent();
        CheckMenuItem(
            GetMenu(frame),
            u32::from(ID_WS_USE_AGENT),
            (MF_BYCOMMAND | if on { MF_CHECKED } else { MF_UNCHECKED }).0,
        );
    }
}

/// ワークスペースのメニューの操作（扱ったら `true`）。
pub(super) fn command(id: u16) -> bool {
    let Some(frame) = with(|a| a.frame) else {
        return false;
    };
    match id {
        ID_SIDEBAR => {
            let Some((visible, tree, grid)) = with(|a| {
                a.sidebar.visible = !a.sidebar.visible;
                a.layout();
                (a.sidebar.visible, a.sidebar.tree, a.grid)
            }) else {
                return true;
            };
            unsafe {
                let _ = SetFocus(Some(if visible { tree } else { grid }));
            }
        }
        ID_WS_NEW => {
            with(|a| {
                a.sidebar.switch_to_untitled();
                a.sidebar.visible = true;
                save(a);
                a.layout();
                a.update_title();
            });
        }
        ID_WS_OPEN => {
            if let Some(file) = crate::app::workspacemode::pick_workspace_file(frame, false, None) {
                match Workspace::load(&file) {
                    Ok(ws) => {
                        with(|a| {
                            a.sidebar.switch_to(Some(file.clone()), ws);
                            a.sidebar.visible = true;
                            a.sidebar.remember(&file);
                            a.layout();
                            a.update_title();
                        });
                    }
                    Err(e) => error_box(frame, &format!("{}\n\n{e}", file.display())),
                }
            }
        }
        ID_WS_SAVE_AS => {
            let current = with(|a| a.sidebar.file.clone()).flatten();
            if let Some(file) =
                crate::app::workspacemode::pick_workspace_file(frame, true, current.as_deref())
            {
                with(|a| {
                    a.sidebar.file = Some(file);
                    save(a);
                    a.update_title();
                });
            }
        }
        ID_WS_ADD => add_folder(frame),
        ID_WS_ADD_REMOTE => add_remote_folder(frame),
        ID_WS_USE_AGENT => {
            crate::remote::set_use_agent(!crate::remote::use_agent());
            check_use_agent(frame);
            set_status(if crate::remote::use_agent() {
                "リモートのフォルダの一覧とファイルの読み書きに、接続先のエージェントを使います"
            } else {
                "リモートのフォルダの一覧とファイルの読み書きに SFTP を使います（接続先に何も置きません）"
            });
        }
        ID_OPEN_REMOTE => open_remote_dialog(frame),
        _ => return false,
    }
    true
}

fn save(a: &mut super::App) {
    if let Err(e) = a.sidebar.save() {
        set_status(&e);
    }
}

/// フォルダをワークスペースに加えて表示する（ドロップされたフォルダなど）。
pub(super) fn add_local_folder(dir: &Path) {
    with(|a| {
        a.sidebar.add_folder(&workspace::normalize(dir));
        save(a);
        a.layout();
    });
}

fn add_folder(frame: HWND) {
    if let Some(dir) = crate::grepdlg::browse_folder(frame) {
        add_local_folder(&dir);
    }
}

fn add_remote_folder(frame: HWND) {
    if !crate::remote::available() {
        info_box(
            frame,
            "この yysheet には SSH の機能が組み込まれていません。",
        );
        return;
    }
    let Some(p) = crate::remotedlg::show(
        frame,
        crate::remotedlg::Mode::Folder,
        crate::remote::last(),
        None,
        false,
    ) else {
        return;
    };
    crate::remote::set_last(p.uri.clone());
    with(|a| {
        a.sidebar.add_folder(&PathBuf::from(p.uri.to_string()));
        save(a);
        a.layout();
    });
}

/// ファイル > リモートのファイルを開く。
fn open_remote_dialog(frame: HWND) {
    if !crate::remote::available() {
        info_box(
            frame,
            "この yysheet には SSH の機能が組み込まれていません。",
        );
        return;
    }
    if !super::confirm_discard() {
        return;
    }
    let Some(p) = crate::remotedlg::show(
        frame,
        crate::remotedlg::Mode::OpenData,
        crate::remote::last(),
        None,
        false,
    ) else {
        return;
    };
    remotefile::open_item(&PathBuf::from(p.uri.to_string()), How::Open, false);
}

/// 項目を開く（ファイルだけ。フォルダは開閉する）。
fn activate(path: &Path, is_dir: bool) -> bool {
    if is_dir {
        return false;
    }
    remotefile::open_item(path, How::Open, true);
    true
}

/// サイドバーの WM_NOTIFY。
pub(super) fn on_notify(hwnd: HWND, hdr: &NMHDR, lparam: LPARAM) -> LRESULT {
    match hdr.code {
        TVN_ITEMEXPANDINGW => {
            let nm = unsafe { &*(lparam.0 as *const NMTREEVIEWW) };
            if nm.action == TVE_EXPAND {
                let pending = with(|a| {
                    a.sidebar
                        .expanding(nm.itemNew.hItem)
                        .map(|i| (a.sidebar.generation, i))
                })
                .flatten();
                if let Some((generation, index)) = pending {
                    // リモートのフォルダは読んでから展開する（ここでは接続しない）
                    unsafe {
                        let _ = PostMessageW(
                            Some(hwnd),
                            WM_APP_SHEET_REMOTE_DIR,
                            WPARAM(generation),
                            LPARAM(index as isize),
                        );
                    }
                    return LRESULT(1);
                }
            }
            LRESULT(0)
        }
        NM_DBLCLK => match with(|a| a.sidebar.selected_node()).flatten() {
            Some((_, path, is_dir, _)) => LRESULT(isize::from(activate(&path, is_dir))),
            None => LRESULT(0),
        },
        TVN_KEYDOWN => {
            let nm = unsafe { &*(lparam.0 as *const NMTVKEYDOWN) };
            if nm.wVKey == VK_RETURN.0 {
                if let Some((_, path, is_dir, _)) = with(|a| a.sidebar.selected_node()).flatten() {
                    activate(&path, is_dir);
                }
                return LRESULT(1);
            }
            if nm.wVKey == VK_DELETE.0 {
                with(|a| {
                    if let Some((_, path, _, true)) = a.sidebar.selected_node() {
                        a.sidebar.remove_folder(&path);
                        save(a);
                    }
                });
                return LRESULT(1);
            }
            LRESULT(0)
        }
        NM_RCLICK => {
            context_menu(hwnd);
            LRESULT(1)
        }
        _ => LRESULT(0),
    }
}

fn context_menu(hwnd: HWND) {
    let Some(pt) = with(|a| a.sidebar.select_at_cursor()) else {
        return;
    };
    let node = with(|a| a.sidebar.selected_node()).flatten();
    let Ok(menu) = (unsafe { CreatePopupMenu() }) else {
        return;
    };
    unsafe {
        let item = |id: u32, text: &str| {
            let _ = AppendMenuW(menu, MF_STRING, id as usize, &HSTRING::from(text));
        };
        let sep = || {
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        };
        if let Some((_, path, is_dir, root)) = &node {
            if *is_dir {
                item(CM_REFRESH, "最新の情報に更新(&R)");
            } else {
                item(CM_OPEN, "開く(&O)");
                item(CM_OPEN_DELIMITED, "区切りを指定して開く(&T)...");
                item(
                    CM_OPEN_MULTI,
                    "固定長ファイル（マルチレイアウト）として開く(&U)",
                );
                item(CM_ADD_FIXED, "固定長ファイルをシートとして追加(&I)");
                item(
                    CM_ADD_MULTI,
                    "固定長ファイルをシートとして追加（マルチレイアウト）(&M)",
                );
                sep();
            }
            if remote_uri(path).is_some() {
                item(CM_DOWNLOAD, "ダウンロード フォルダにコピー(&L)");
            }
            item(CM_COPY_PATH, "パスをコピー(&C)");
            if *root {
                item(CM_REMOVE, "ワークスペースから外す(&D)");
            }
            sep();
        }
        item(CM_ADD, "フォルダを追加(&F)...");
        item(CM_ADD_REMOTE, "リモートのフォルダを追加(&S)...");
        let cmd = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON,
            pt.x,
            pt.y,
            None,
            hwnd,
            None,
        );
        let _ = DestroyMenu(menu);
        let open = |how: How| {
            if let Some((_, path, _, _)) = &node {
                remotefile::open_item(path, how, true);
            }
        };
        match cmd.0 as u32 {
            CM_OPEN => open(How::Open),
            CM_OPEN_MULTI => open(How::OpenMulti),
            CM_OPEN_DELIMITED => open(How::OpenDelimited),
            CM_ADD_FIXED => open(How::AddFixed),
            CM_ADD_MULTI => open(How::AddMulti),
            CM_ADD => add_folder(hwnd),
            CM_ADD_REMOTE => add_remote_folder(hwnd),
            CM_DOWNLOAD => {
                if let Some(u) = node.as_ref().and_then(|n| remote_uri(&n.1)) {
                    match crate::download::download(hwnd, &u) {
                        Ok(Some(m)) => set_status(&m),
                        Ok(None) => {}
                        Err(e) => error_box(hwnd, &e),
                    }
                }
            }
            CM_COPY_PATH => {
                if let Some((_, path, _, _)) = &node {
                    let _ = crate::clipboard::set_text(hwnd, &path.to_string_lossy(), false);
                }
            }
            CM_REFRESH => {
                with(|a| a.sidebar.rebuild());
            }
            CM_REMOVE => {
                if let Some((_, path, _, _)) = &node {
                    with(|a| {
                        a.sidebar.remove_folder(path);
                        save(a);
                    });
                }
            }
            _ => {}
        }
    }
}

/// [`WM_APP_SHEET_REMOTE_DIR`] を処理する。
pub(super) fn load_remote_dir(hwnd: HWND, generation: usize, index: usize) {
    crate::wsbar::load_remote_dir(hwnd, generation, index, &|f| {
        with(|a| f(&mut a.sidebar));
    });
}
