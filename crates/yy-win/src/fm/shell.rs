//! 「フォルダ」のタブが使う Windows のシェルの機能（2026-10-27 の提案 4 章）。
//!
//! コピー・移動・削除・名前の変更はエクスプローラーと同じ `IFileOperation`（進み・衝突の確かめ・
//! ごみ箱・取り消し）。右クリックのシェルのメニュー（`IContextMenu`。「送る」などの子メニューは
//! `IContextMenu3` に渡す）、ファイルのクリップボード（`CF_HDROP` と「優先する動作」）、
//! ドラッグで外へ出す（`SHDoDragDrop`）、プロパティ・プログラムから開く・よく使うフォルダ。

use std::cell::RefCell;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{
    GlobalFree, HANDLE, HGLOBAL, HWND, LPARAM, LRESULT, POINT, WPARAM,
};
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance, CoTaskMemFree, IDataObject};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{
    GMEM_MOVEABLE, GMEM_ZEROINIT, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
};
use windows::Win32::System::Ole::{
    CF_HDROP, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_MOVE,
};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, Interface, PCSTR, PCWSTR, w};

fn wide_path(p: &Path) -> Vec<u16> {
    p.as_os_str().encode_wide().chain(Some(0)).collect()
}

fn shell_item(p: &Path) -> windows::core::Result<IShellItem> {
    unsafe { SHCreateItemFromParsingName(&HSTRING::from(p.as_os_str()), None) }
}

/// ファイル操作の種類。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Op {
    Copy,
    Move,
    /// ごみ箱へ
    Recycle,
    /// 完全に削除
    Delete,
}

/// `IFileOperation` で操作する（`dest` はコピー・移動の送り先のフォルダ）。進みと確かめは Windows の
/// 画面が出す。中止されたら `Ok(false)`。
pub(super) fn file_op(
    owner: HWND,
    op: Op,
    items: &[PathBuf],
    dest: Option<&Path>,
) -> Result<bool, String> {
    if items.is_empty() {
        return Ok(true);
    }
    unsafe {
        let fo: IFileOperation =
            CoCreateInstance(&FileOperation, None, CLSCTX_ALL).map_err(|e| e.message())?;
        let _ = fo.SetOwnerWindow(owner);
        let mut flags = FOF_NOCONFIRMMKDIR.0 | FOFX_ADDUNDORECORD.0;
        match op {
            Op::Recycle => flags |= FOF_ALLOWUNDO.0 | FOFX_RECYCLEONDELETE.0,
            Op::Copy | Op::Move => flags |= FOF_ALLOWUNDO.0,
            Op::Delete => {}
        }
        fo.SetOperationFlags(FILEOPERATION_FLAGS(flags))
            .map_err(|e| e.message())?;
        let dest_item = match dest {
            Some(d) => {
                Some(shell_item(d).map_err(|e| format!("{}: {}", d.display(), e.message()))?)
            }
            None => None,
        };
        for p in items {
            let it = shell_item(p).map_err(|e| format!("{}: {}", p.display(), e.message()))?;
            let r = match (op, &dest_item) {
                (Op::Copy, Some(d)) => fo.CopyItem(&it, d, PCWSTR::null(), None),
                (Op::Move, Some(d)) => fo.MoveItem(&it, d, PCWSTR::null(), None),
                (Op::Recycle | Op::Delete, _) => fo.DeleteItem(&it, None),
                _ => return Err("送り先がありません".into()),
            };
            r.map_err(|e| format!("{}: {}", p.display(), e.message()))?;
        }
        fo.PerformOperations().map_err(|e| e.message())?;
        Ok(!fo
            .GetAnyOperationsAborted()
            .map(|b| b.as_bool())
            .unwrap_or(false))
    }
}

/// 名前を変える（`IFileOperation`。取り消しの記録に残る）。
pub(super) fn rename(owner: HWND, path: &Path, new_name: &str) -> Result<(), String> {
    unsafe {
        let fo: IFileOperation =
            CoCreateInstance(&FileOperation, None, CLSCTX_ALL).map_err(|e| e.message())?;
        let _ = fo.SetOwnerWindow(owner);
        fo.SetOperationFlags(FILEOPERATION_FLAGS(FOF_ALLOWUNDO.0 | FOFX_ADDUNDORECORD.0))
            .map_err(|e| e.message())?;
        let it = shell_item(path).map_err(|e| e.message())?;
        fo.RenameItem(&it, &HSTRING::from(new_name), None)
            .map_err(|e| e.message())?;
        fo.PerformOperations().map_err(|e| e.message())?;
        if fo
            .GetAnyOperationsAborted()
            .map(|b| b.as_bool())
            .unwrap_or(false)
        {
            return Err("中止しました".into());
        }
        Ok(())
    }
}

// ---- ファイルのクリップボード ----

fn drop_effect_format() -> u32 {
    unsafe { RegisterClipboardFormatW(w!("Preferred DropEffect")) }
}

/// `CF_HDROP` の中身（`DROPFILES` と、NUL で区切り 2 つの NUL で終わるパスの並び）。
fn hdrop_global(paths: &[PathBuf]) -> Option<HGLOBAL> {
    let mut list: Vec<u16> = Vec::new();
    for p in paths {
        list.extend(p.as_os_str().encode_wide());
        list.push(0);
    }
    list.push(0);
    let head = std::mem::size_of::<DROPFILES>();
    let size = head + list.len() * 2;
    unsafe {
        let h = GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, size).ok()?;
        let p = GlobalLock(h) as *mut u8;
        if p.is_null() {
            let _ = GlobalFree(Some(h));
            return None;
        }
        let df = DROPFILES {
            pFiles: head as u32,
            fWide: true.into(),
            ..Default::default()
        };
        std::ptr::copy_nonoverlapping(&df as *const DROPFILES as *const u8, p, head);
        std::ptr::copy_nonoverlapping(list.as_ptr() as *const u8, p.add(head), list.len() * 2);
        let _ = GlobalUnlock(h);
        Some(h)
    }
}

fn dword_global(v: u32) -> Option<HGLOBAL> {
    unsafe {
        let h = GlobalAlloc(GMEM_MOVEABLE, 4).ok()?;
        let p = GlobalLock(h) as *mut u32;
        if p.is_null() {
            let _ = GlobalFree(Some(h));
            return None;
        }
        *p = v;
        let _ = GlobalUnlock(h);
        Some(h)
    }
}

/// ファイルをクリップボードに置く（`cut` なら切り取り）。エクスプローラーに貼り付けられる。
pub(super) fn set_clipboard_files(owner: HWND, paths: &[PathBuf], cut: bool) -> bool {
    let (Some(files), Some(effect)) = (
        hdrop_global(paths),
        dword_global(if cut {
            DROPEFFECT_MOVE.0
        } else {
            DROPEFFECT_COPY.0 | DROPEFFECT_LINK.0
        }),
    ) else {
        return false;
    };
    unsafe {
        if OpenClipboard(Some(owner)).is_err() {
            let _ = GlobalFree(Some(files));
            let _ = GlobalFree(Some(effect));
            return false;
        }
        let _ = EmptyClipboard();
        let ok = SetClipboardData(CF_HDROP.0 as u32, Some(HANDLE(files.0))).is_ok();
        let _ = SetClipboardData(drop_effect_format(), Some(HANDLE(effect.0)));
        let _ = CloseClipboard();
        ok
    }
}

/// `HDROP` のパス。
pub(super) fn hdrop_paths(drop: HDROP) -> Vec<PathBuf> {
    let mut out = Vec::new();
    unsafe {
        let n = DragQueryFileW(drop, u32::MAX, None);
        for i in 0..n {
            let len = DragQueryFileW(drop, i, None) as usize;
            let mut buf = vec![0u16; len + 1];
            let got = DragQueryFileW(drop, i, Some(&mut buf)) as usize;
            out.push(PathBuf::from(String::from_utf16_lossy(&buf[..got])));
        }
    }
    out
}

/// クリップボードのファイル（パス・切り取りか）。
pub(super) fn clipboard_files(owner: HWND) -> Option<(Vec<PathBuf>, bool)> {
    unsafe {
        if IsClipboardFormatAvailable(CF_HDROP.0 as u32).is_err() {
            return None;
        }
        OpenClipboard(Some(owner)).ok()?;
        let res = (|| {
            let h = GetClipboardData(CF_HDROP.0 as u32).ok()?;
            let paths = hdrop_paths(HDROP(h.0));
            let mut cut = false;
            if let Ok(e) = GetClipboardData(drop_effect_format()) {
                let g = HGLOBAL(e.0);
                let p = GlobalLock(g) as *const u32;
                if !p.is_null() && GlobalSize(g) >= 4 {
                    cut = *p & DROPEFFECT_MOVE.0 != 0 && *p & DROPEFFECT_COPY.0 == 0;
                }
                let _ = GlobalUnlock(g);
            }
            Some((paths, cut))
        })();
        let _ = CloseClipboard();
        res
    }
}

/// クリップボードにファイルがあるか。
pub(super) fn has_clipboard_files() -> bool {
    unsafe { IsClipboardFormatAvailable(CF_HDROP.0 as u32).is_ok() }
}

/// 切り取りを貼り付けたあと、クリップボードを空にする（エクスプローラーと同じ）。
pub(super) fn clear_clipboard(owner: HWND) {
    unsafe {
        if OpenClipboard(Some(owner)).is_ok() {
            let _ = EmptyClipboard();
            let _ = CloseClipboard();
        }
    }
}

// ---- ドラッグで外へ ----

/// ファイルを外へドラッグする（エクスプローラー・他のアプリへ）。済んだ動作を返す。
pub(super) fn drag_out(owner: HWND, paths: &[PathBuf]) -> Option<DROPEFFECT> {
    unsafe {
        // デスクトップからの PIDL の並び（違うフォルダの項目が混じってもよい）
        let pidls = Pidls(
            paths
                .iter()
                .map(|p| ILCreateFromPathW(PCWSTR(wide_path(p).as_ptr())))
                .collect(),
        );
        if pidls.0.is_empty() || pidls.0.iter().any(|p| p.is_null()) {
            return None;
        }
        let ids: Vec<*const ITEMIDLIST> = pidls.0.iter().map(|p| *p as *const _).collect();
        let data: IDataObject = SHCreateDataObject(None, Some(&ids), None).ok()?;
        SHDoDragDrop(
            Some(owner),
            &data,
            None,
            DROPEFFECT_COPY | DROPEFFECT_MOVE | DROPEFFECT_LINK,
        )
        .ok()
    }
}

// ---- 右クリックのシェルのメニュー ----

/// シェルのメニューの番号の始まり（これより小さい番号は yyfilemanager の項目）
pub(super) const SHELL_FIRST: u32 = 1000;
const SHELL_LAST: u32 = 0x7fff;

thread_local! {
    /// 開いているシェルのメニュー（子メニューの描画を渡す）
    static ACTIVE: RefCell<Option<IContextMenu>> = const { RefCell::new(None) };
}

/// メニューを出しているあいだの、親のウィンドウのメッセージ（子メニューの描画など）。処理したら `Some`。
pub(super) fn menu_message(msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
    if !matches!(
        msg,
        WM_INITMENUPOPUP | WM_DRAWITEM | WM_MEASUREITEM | WM_MENUCHAR
    ) {
        return None;
    }
    ACTIVE.with(|a| {
        let a = a.borrow();
        let cm = a.as_ref()?;
        unsafe {
            if let Ok(c3) = cm.cast::<IContextMenu3>() {
                let mut r = LRESULT(0);
                if c3.HandleMenuMsg2(msg, wparam, lparam, Some(&mut r)).is_ok() {
                    return Some(r);
                }
            } else if let Ok(c2) = cm.cast::<IContextMenu2>()
                && c2.HandleMenuMsg(msg, wparam, lparam).is_ok()
            {
                return Some(LRESULT(0));
            }
        }
        None
    })
}

/// パスの PIDL（使い終わったら `CoTaskMemFree`）。
struct Pidls(Vec<*mut ITEMIDLIST>);

impl Drop for Pidls {
    fn drop(&mut self) {
        for p in &self.0 {
            unsafe { ILFree(Some(*p)) };
        }
    }
}

/// 同じフォルダの項目のシェルのメニュー（違うフォルダが混じれば `None`）。
fn context_menu_of(owner: HWND, paths: &[PathBuf]) -> Option<IContextMenu> {
    let parent = paths.first()?.parent()?.to_path_buf();
    let lower = |p: &Path| p.to_string_lossy().to_lowercase();
    if paths
        .iter()
        .any(|p| p.parent().map(lower) != Some(lower(&parent)))
    {
        return None;
    }
    unsafe {
        let pidls = Pidls(
            paths
                .iter()
                .map(|p| ILCreateFromPathW(PCWSTR(wide_path(p).as_ptr())))
                .collect(),
        );
        if pidls.0.iter().any(|p| p.is_null()) {
            return None;
        }
        let folder: IShellFolder = SHBindToParent(pidls.0[0], None).ok()?;
        let children: Vec<*const ITEMIDLIST> = pidls
            .0
            .iter()
            .map(|p| ILFindLastID(*p) as *const _)
            .collect();
        folder.GetUIObjectOf(owner, &children, None).ok()
    }
}

/// シェルのメニューを `menu` の終わりに足す（違うフォルダが混じれば足さない）。
pub(super) fn add_shell_menu(owner: HWND, menu: HMENU, paths: &[PathBuf], extended: bool) -> bool {
    let Some(cm) = context_menu_of(owner, paths) else {
        return false;
    };
    let mut flags = CMF_NORMAL | CMF_CANRENAME;
    if extended {
        flags |= CMF_EXTENDEDVERBS;
    }
    let pos = unsafe { GetMenuItemCount(Some(menu)) }.max(0) as u32;
    if unsafe { cm.QueryContextMenu(menu, pos, SHELL_FIRST, SHELL_LAST, flags) }.is_err() {
        return false;
    }
    ACTIVE.with(|a| *a.borrow_mut() = Some(cm));
    true
}

/// シェルのメニューの番号の動作の名前（`rename`・`delete` など）。
pub(super) fn shell_verb(cmd: u32) -> Option<String> {
    ACTIVE.with(|a| {
        let a = a.borrow();
        let cm = a.as_ref()?;
        let mut buf = [0u8; 128];
        unsafe {
            cm.GetCommandString(
                (cmd - SHELL_FIRST) as usize,
                GCS_VERBA,
                None,
                PSTR(buf.as_mut_ptr()),
                buf.len() as u32,
            )
            .ok()?;
        }
        let n = buf.iter().position(|&c| c == 0).unwrap_or(0);
        Some(String::from_utf8_lossy(&buf[..n]).into_owned())
    })
}

use windows::core::PSTR;

/// シェルのメニューの項目を実行する。
pub(super) fn invoke_shell(owner: HWND, cmd: u32, at: POINT) {
    ACTIVE.with(|a| {
        if let Some(cm) = a.borrow().as_ref() {
            invoke(
                cm,
                owner,
                PCSTR((cmd - SHELL_FIRST) as usize as *const u8),
                at,
            );
        }
    });
}

/// シェルのメニューを閉じる（`ACTIVE` を外す）。
pub(super) fn end_shell_menu() {
    ACTIVE.with(|a| *a.borrow_mut() = None);
}

fn invoke(cm: &IContextMenu, owner: HWND, verb: PCSTR, at: POINT) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_SHIFT};
    let mut mask = CMIC_MASK_PTINVOKE;
    unsafe {
        if GetKeyState(VK_SHIFT.0 as i32) < 0 {
            mask |= CMIC_MASK_SHIFT_DOWN;
        }
        if GetKeyState(VK_CONTROL.0 as i32) < 0 {
            mask |= CMIC_MASK_CONTROL_DOWN;
        }
    }
    let info = CMINVOKECOMMANDINFOEX {
        cbSize: std::mem::size_of::<CMINVOKECOMMANDINFOEX>() as u32,
        fMask: mask,
        hwnd: owner,
        lpVerb: verb,
        nShow: SW_SHOWNORMAL.0,
        ptInvoke: at,
        ..Default::default()
    };
    unsafe {
        let _ =
            cm.InvokeCommand(&info as *const CMINVOKECOMMANDINFOEX as *const CMINVOKECOMMANDINFO);
    }
}

/// プロパティを出す（同じフォルダの複数なら 1 つの画面に。違うフォルダなら最初の 1 つ）。
pub(super) fn properties(owner: HWND, paths: &[PathBuf]) {
    if paths.len() > 1
        && let Some(cm) = context_menu_of(owner, paths)
    {
        unsafe {
            let m = CreatePopupMenu().ok();
            if let Some(m) = m {
                let _ = cm.QueryContextMenu(m, 0, SHELL_FIRST, SHELL_LAST, CMF_NORMAL);
                invoke(
                    &cm,
                    owner,
                    windows::core::s!("properties"),
                    POINT::default(),
                );
                let _ = DestroyMenu(m);
            }
        }
        return;
    }
    if let Some(p) = paths.first() {
        unsafe {
            let _ = SHObjectProperties(
                Some(owner),
                SHOP_FILEPATH,
                &HSTRING::from(p.as_os_str()),
                None,
            );
        }
    }
}

/// 「プログラムから開く」の画面。
pub(super) fn open_with(owner: HWND, path: &Path) {
    let p = wide_path(path);
    let info = OPENASINFO {
        pcszFile: PCWSTR(p.as_ptr()),
        pcszClass: PCWSTR::null(),
        oaifInFlags: OAIF_ALLOW_REGISTRATION | OAIF_REGISTER_EXT | OAIF_EXEC,
    };
    unsafe {
        let _ = SHOpenWithDialog(Some(owner), &info);
    }
}

/// よく使うフォルダ（デスクトップ・ダウンロード・ドキュメント・ピクチャ・ミュージック・ビデオ）。
pub(super) fn known_folders() -> Vec<(String, PathBuf)> {
    let ids = [
        ("デスクトップ", FOLDERID_Desktop),
        ("ダウンロード", FOLDERID_Downloads),
        ("ドキュメント", FOLDERID_Documents),
        ("ピクチャ", FOLDERID_Pictures),
        ("ミュージック", FOLDERID_Music),
        ("ビデオ", FOLDERID_Videos),
    ];
    let mut out = Vec::new();
    for (name, id) in ids {
        unsafe {
            if let Ok(p) = SHGetKnownFolderPath(&id, KF_FLAG_DEFAULT, None) {
                if let Ok(s) = p.to_string() {
                    out.push((name.to_owned(), PathBuf::from(s)));
                }
                CoTaskMemFree(Some(p.0 as *const _));
            }
        }
    }
    out
}

/// 表示名（ドライブのラベルなど）。
pub(super) fn display_name(path: &Path) -> String {
    let p = wide_path(path);
    let mut info = SHFILEINFOW::default();
    unsafe {
        SHGetFileInfoW(
            PCWSTR(p.as_ptr()),
            Default::default(),
            Some(&mut info),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_DISPLAYNAME,
        );
    }
    let n = info.szDisplayName.iter().position(|&c| c == 0).unwrap_or(0);
    let s = String::from_utf16_lossy(&info.szDisplayName[..n]);
    if s.is_empty() {
        path.to_string_lossy().into_owned()
    } else {
        s
    }
}

/// アドレスバーにパスの補完を付ける。
pub(super) fn autocomplete(edit: HWND) {
    unsafe {
        let _ = SHAutoComplete(edit, SHACF_FILESYS_DIRS | SHACF_AUTOSUGGEST_FORCE_ON);
    }
}
