//! 左のブックマークの欄（19 章 4.1）: yyeditor のワークスペースと同じく、左にツリーで並べ、境界を
//! ドラッグして幅を変え、表示・非表示を切り替える（ブックマーク メニュー・Ctrl+Shift+B・📑 のボタン）。
//!
//! フォルダは折りたためるノード、ブックマークはその下の項目。ダブルクリック・Enter で開き、中ボタン・
//! Ctrl+Enter で新しいタブに開く。右クリックで開く・編集・期限付きの切り替え・削除。期限付きのものには
//! 残りの日数を添える。

use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL};
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::Shell::{
    SHFILEINFOW, SHGFI_SMALLICON, SHGFI_SYSICONINDEX, SHGFI_USEFILEATTRIBUTES, SHGetFileInfoW,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, w};
use yy_browser::bookmarks::{Bookmarks, Node};

use super::ID_BM_TREE;

/// 欄の既定の幅（96 DPI でのピクセル）
pub(super) const DEFAULT_WIDTH: i32 = 240;
/// 境界の幅（96 DPI でのピクセル）
pub(super) const SPLITTER: i32 = 5;

/// ツリーの項目の `lParam`: フォルダは 0、ブックマークは番号 + 1。
fn param_of(index: usize) -> isize {
    index as isize + 1
}

/// ツリーを作る（初めは隠しておく）。
pub(super) fn create(frame: HWND, instance: windows::Win32::Foundation::HINSTANCE) -> HWND {
    unsafe {
        let tree = CreateWindowExW(
            WS_EX_CLIENTEDGE,
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
            Some(HMENU(ID_BM_TREE as usize as *mut _)),
            Some(instance),
            None,
        )
        .unwrap_or_default();
        let _ = SetWindowTheme(tree, w!("Explorer"), None);
        SendMessageW(
            tree,
            TVM_SETEXTENDEDSTYLE,
            Some(WPARAM(TVS_EX_DOUBLEBUFFER as usize)),
            Some(LPARAM(TVS_EX_DOUBLEBUFFER as isize)),
        );
        // システムの小さいアイコン（フォルダとインターネットのショートカット）
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
        tree
    }
}

fn icon(
    name: windows::core::PCWSTR,
    attrs: windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES,
) -> i32 {
    let mut info = SHFILEINFOW::default();
    unsafe {
        SHGetFileInfoW(
            name,
            attrs,
            Some(&mut info),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_SYSICONINDEX | SHGFI_SMALLICON | SHGFI_USEFILEATTRIBUTES,
        );
    }
    info.iIcon
}

/// ブックマークの表示名（期限付きなら残りの日数を添える）。
pub(super) fn label(b: &yy_browser::bookmarks::Bookmark, now: u64) -> String {
    match b.days_left(now) {
        Some(d) => format!("{}（あと {d} 日）", b.label()),
        None => b.label().to_owned(),
    }
}

fn insert(
    tree: HWND,
    parent: HTREEITEM,
    text: &str,
    param: isize,
    image: i32,
    folder: bool,
) -> HTREEITEM {
    let mut text: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    let ins = TVINSERTSTRUCTW {
        hParent: parent,
        hInsertAfter: TVI_LAST,
        Anonymous: TVINSERTSTRUCTW_0 {
            itemex: TVITEMEXW {
                mask: TVIF_TEXT | TVIF_PARAM | TVIF_IMAGE | TVIF_SELECTEDIMAGE | TVIF_STATE,
                pszText: windows::core::PWSTR(text.as_mut_ptr()),
                lParam: LPARAM(param),
                iImage: image,
                iSelectedImage: image,
                state: if folder { TVIS_EXPANDED.0 } else { 0 },
                stateMask: TVIS_EXPANDED.0,
                ..Default::default()
            },
        },
    };
    unsafe {
        HTREEITEM(
            SendMessageW(
                tree,
                TVM_INSERTITEMW,
                None,
                Some(LPARAM(&ins as *const _ as isize)),
            )
            .0,
        )
    }
}

/// ツリーを作り直す（フォルダは開いておく）。
pub(super) fn fill(tree: HWND, list: &Bookmarks, now: u64) {
    fn add(tree: HWND, parent: HTREEITEM, list: &Bookmarks, n: &Node, now: u64, icons: (i32, i32)) {
        for (name, child) in &n.folders {
            let item = insert(tree, parent, name, 0, icons.0, true);
            add(tree, item, list, child, now, icons);
            unsafe {
                SendMessageW(
                    tree,
                    TVM_EXPAND,
                    Some(WPARAM(TVE_EXPAND.0 as usize)),
                    Some(LPARAM(item.0)),
                );
            }
        }
        for &i in &n.items {
            insert(
                tree,
                parent,
                &label(&list.items[i], now),
                param_of(i),
                icons.1,
                false,
            );
        }
    }
    let icons = (
        icon(w!("folder"), FILE_ATTRIBUTE_DIRECTORY),
        icon(w!("bookmark.url"), FILE_ATTRIBUTE_NORMAL),
    );
    unsafe {
        SendMessageW(tree, WM_SETREDRAW, Some(WPARAM(0)), None);
        SendMessageW(tree, TVM_DELETEITEM, None, Some(LPARAM(TVI_ROOT.0)));
    }
    add(tree, TVI_ROOT, list, &list.tree(), now, icons);
    if list.items.is_empty() {
        insert(
            tree,
            TVI_ROOT,
            "（ブックマークはまだありません。Ctrl+D で追加）",
            0,
            icons.1,
            false,
        );
    }
    unsafe {
        SendMessageW(tree, WM_SETREDRAW, Some(WPARAM(1)), None);
        let _ = windows::Win32::Graphics::Gdi::InvalidateRect(Some(tree), None, true);
    }
}

/// 項目のブックマークの番号（フォルダ・空の表示なら `None`）。
pub(super) fn index_of(tree: HWND, item: HTREEITEM) -> Option<usize> {
    if item.0 == 0 {
        return None;
    }
    let mut it = TVITEMW {
        mask: TVIF_PARAM | TVIF_HANDLE,
        hItem: item,
        ..Default::default()
    };
    unsafe {
        SendMessageW(
            tree,
            TVM_GETITEMW,
            None,
            Some(LPARAM(&mut it as *mut _ as isize)),
        );
    }
    (it.lParam.0 > 0).then(|| it.lParam.0 as usize - 1)
}

/// 選んでいる項目。
pub(super) fn selected(tree: HWND) -> HTREEITEM {
    unsafe {
        HTREEITEM(
            SendMessageW(
                tree,
                TVM_GETNEXTITEM,
                Some(WPARAM(TVGN_CARET as usize)),
                None,
            )
            .0,
        )
    }
}

/// マウスの下の項目を選んで返す（右クリック・中ボタン）。画面の座標も返す。
pub(super) fn item_at_cursor(tree: HWND) -> (HTREEITEM, POINT) {
    let mut screen = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut screen);
    }
    let mut pt = screen;
    unsafe {
        let _ = windows::Win32::Graphics::Gdi::ScreenToClient(tree, &mut pt);
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
    if item.0 != 0 {
        unsafe {
            SendMessageW(
                tree,
                TVM_SELECTITEM,
                Some(WPARAM(TVGN_CARET as usize)),
                Some(LPARAM(item.0)),
            );
        }
    }
    (item, screen)
}

/// 右クリックのメニューで選んだもの。
pub(super) enum Choice {
    Open,
    OpenTab,
    Edit,
    ToggleExpiry,
    Delete,
    Hide,
}

/// 右クリックのメニュー（ブックマークの上なら開く・編集など、それ以外は隠すだけ）。
pub(super) fn context_menu(
    owner: HWND,
    at: POINT,
    on_bookmark: bool,
    expiring: bool,
) -> Option<Choice> {
    const C_OPEN: usize = 1;
    const C_TAB: usize = 2;
    const C_EDIT: usize = 3;
    const C_EXPIRY: usize = 4;
    const C_DELETE: usize = 5;
    const C_HIDE: usize = 6;
    unsafe {
        let m = CreatePopupMenu().ok()?;
        let add = |id: usize, t: &str, flags: MENU_ITEM_FLAGS| {
            let _ = AppendMenuW(m, MF_STRING | flags, id, &HSTRING::from(t));
        };
        if on_bookmark {
            add(C_OPEN, "開く(&O)\tEnter", MENU_ITEM_FLAGS(0));
            add(
                C_TAB,
                "新しいタブで開く(&T)\tCtrl+Enter",
                MENU_ITEM_FLAGS(0),
            );
            let _ = AppendMenuW(m, MF_SEPARATOR, 0, None);
            add(C_EDIT, "編集(&E)...", MENU_ITEM_FLAGS(0));
            add(
                C_EXPIRY,
                "1 か月で消す（使うたびに延長）(&X)",
                if expiring {
                    MF_CHECKED
                } else {
                    MENU_ITEM_FLAGS(0)
                },
            );
            add(C_DELETE, "削除(&D)\tDel", MENU_ITEM_FLAGS(0));
            let _ = AppendMenuW(m, MF_SEPARATOR, 0, None);
        }
        add(
            C_HIDE,
            "ブックマークの欄を閉じる(&H)\tCtrl+Shift+B",
            MENU_ITEM_FLAGS(0),
        );
        let cmd = TrackPopupMenu(
            m,
            TPM_RETURNCMD | TPM_RIGHTBUTTON,
            at.x,
            at.y,
            None,
            owner,
            None,
        );
        let _ = DestroyMenu(m);
        Some(match cmd.0 as usize {
            C_OPEN => Choice::Open,
            C_TAB => Choice::OpenTab,
            C_EDIT => Choice::Edit,
            C_EXPIRY => Choice::ToggleExpiry,
            C_DELETE => Choice::Delete,
            C_HIDE => Choice::Hide,
            _ => return None,
        })
    }
}

/// 境界の上か（フレームの座標）。
pub(super) fn on_splitter(rc: &RECT, x: i32, y: i32) -> bool {
    rc.right > rc.left && x >= rc.left && x < rc.right && y >= rc.top && y < rc.bottom
}
