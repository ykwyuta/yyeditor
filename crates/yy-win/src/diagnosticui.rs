//! Shared opt-in SSH diagnostic controls. Export only the structured recorder.
use crate::util::{error_box, info_box};
use windows::{
    Win32::{Foundation::HWND, UI::WindowsAndMessaging::*},
    core::{HSTRING, w},
};
use yy_remote::diagnostics;
const START: u16 = 62004;
const STOP: u16 = 62005;
const EXPORT: u16 = 62006;
pub(crate) fn append_menu(menu: HMENU) -> windows::core::Result<()> {
    unsafe {
        AppendMenuW(menu, MF_SEPARATOR, 0, None)?;
        AppendMenuW(menu, MF_STRING, START as usize, w!("SSH診断の記録を開始"))?;
        AppendMenuW(menu, MF_STRING, STOP as usize, w!("SSH診断の記録を停止"))?;
        AppendMenuW(
            menu,
            MF_STRING,
            EXPORT as usize,
            w!("SSH診断情報を書き出す…"),
        )?;
    }
    Ok(())
}
pub(crate) fn command(owner: HWND, id: u16) -> bool {
    match id {
        START => {
            let directory = std::env::var_os("APPDATA")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(std::env::temp_dir)
                .join("yyeditor")
                .join("logs");
            match diagnostics::start(
                &directory,
                crate::util::app_name(),
                env!("CARGO_PKG_VERSION"),
            ) {
                Ok(path) => info_box(
                    owner,
                    &format!(
                        "SSH診断を記録しています。\n接続し直して問題を再現し、記録を停止して書き出してください。\n\n保存先: {}\n\nユーザー名・ホスト名・パス・コマンド・パスワード・通信本文は記録しません。上限は8192イベントまたは4 MiBです。",
                        path.display()
                    ),
                ),
                Err(e) => error_box(owner, &format!("診断記録を開始できません: {e}")),
            }
        }
        STOP => {
            diagnostics::stop();
            info_box(
                owner,
                "SSH診断の記録を停止しました。記録済みの情報は書き出せます。",
            );
        }
        EXPORT => {
            let Some(report) = diagnostics::report() else {
                info_box(owner, "先にSSH診断の記録を開始し、問題を再現してください。");
                return true;
            };
            if let Some(path) = pick_file(owner) {
                let result = (|| -> std::io::Result<()> {
                    if diagnostics::path().is_some_and(|source| {
                        source
                            .canonicalize()
                            .ok()
                            .zip(path.canonicalize().ok())
                            .is_some_and(|(a, b)| a == b)
                    }) {
                        return Err(std::io::Error::other(
                            "記録元とは別のファイル名を指定してください",
                        ));
                    }
                    use std::io::Write;
                    let mut file = tempfile::NamedTempFile::new_in(
                        path.parent().unwrap_or(std::path::Path::new(".")),
                    )?;
                    file.write_all(report.as_bytes())?;
                    file.as_file().sync_all()?;
                    file.persist(&path).map_err(|e| e.error)?;
                    Ok(())
                })();
                match result {
                    Ok(()) => info_box(owner, "SSH診断情報を書き出しました。"),
                    Err(e) => error_box(owner, &format!("診断情報を書き出せません: {e}")),
                }
            }
        }
        _ => return false,
    }
    true
}
fn pick_file(owner: HWND) -> Option<std::path::PathBuf> {
    use windows::Win32::{
        System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree},
        UI::Shell::{
            FOS_FORCEFILESYSTEM, FOS_OVERWRITEPROMPT, FOS_PATHMUSTEXIST, FileSaveDialog,
            IFileSaveDialog, SIGDN_FILESYSPATH,
        },
    };
    unsafe {
        let dialog: IFileSaveDialog =
            CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        dialog
            .SetOptions(
                dialog.GetOptions().ok()?
                    | FOS_FORCEFILESYSTEM
                    | FOS_PATHMUSTEXIST
                    | FOS_OVERWRITEPROMPT,
            )
            .ok()?;
        dialog.SetTitle(w!("SSH診断情報を書き出す")).ok()?;
        dialog.SetDefaultExtension(w!("txt")).ok()?;
        dialog
            .SetFileName(&HSTRING::from("yy-ssh-diagnostic.txt"))
            .ok()?;
        dialog.Show(Some(owner)).ok()?;
        let name = dialog
            .GetResult()
            .ok()?
            .GetDisplayName(SIGDN_FILESYSPATH)
            .ok()?;
        let result = name.to_string().ok().map(std::path::PathBuf::from);
        CoTaskMemFree(Some(name.0 as *const _));
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_help_menu_has_distinct_diagnostic_commands() {
        unsafe {
            let menu = CreatePopupMenu().unwrap();
            append_menu(menu).unwrap();
            assert_eq!(GetMenuItemCount(Some(menu)), 4);
            for (position, id) in [(1, START), (2, STOP), (3, EXPORT)] {
                assert_eq!(GetMenuItemID(menu, position), id as u32);
            }
            DestroyMenu(menu).unwrap();
        }
    }
}
