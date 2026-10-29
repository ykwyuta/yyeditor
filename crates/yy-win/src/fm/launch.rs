//! Explicit consent for unsigned PE applications; OS execution policy remains effective.
use std::fs::OpenOptions;
use std::io;
use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
use std::path::Path;
use std::process::{Child, Command};
use windows::Win32::Foundation::{HANDLE, HWND};
use windows::Win32::Security::WinTrust::*;
use windows::Win32::Storage::FileSystem::FILE_SHARE_READ;
use windows::Win32::UI::{Shell::*, WindowsAndMessaging::*};
use windows::core::{HSTRING, PCWSTR, w};

fn trusted(path: &Path, file: &std::fs::File) -> i32 {
    unsafe {
        let path = HSTRING::from(path.as_os_str());
        let mut info = WINTRUST_FILE_INFO {
            cbStruct: std::mem::size_of::<WINTRUST_FILE_INFO>() as u32,
            pcwszFilePath: PCWSTR(path.as_ptr()),
            hFile: HANDLE(file.as_raw_handle()),
            ..Default::default()
        };
        let mut data = WINTRUST_DATA {
            cbStruct: std::mem::size_of::<WINTRUST_DATA>() as u32,
            dwUIChoice: WTD_UI_NONE,
            fdwRevocationChecks: WTD_REVOKE_WHOLECHAIN,
            dwUnionChoice: WTD_CHOICE_FILE,
            Anonymous: WINTRUST_DATA_0 { pFile: &mut info },
            dwStateAction: WTD_STATEACTION_VERIFY,
            dwProvFlags: WTD_CACHE_ONLY_URL_RETRIEVAL | WTD_REVOCATION_CHECK_CHAIN_EXCLUDE_ROOT,
            ..Default::default()
        };
        let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
        let status = WinVerifyTrust(
            HWND::default(),
            &mut action,
            (&mut data as *mut WINTRUST_DATA).cast(),
        );
        data.dwStateAction = WTD_STATEACTION_CLOSE;
        let _ = WinVerifyTrust(
            HWND::default(),
            &mut action,
            (&mut data as *mut WINTRUST_DATA).cast(),
        );
        status
    }
}

/// Hold a read handle denying write/delete from signature check through approval
/// and process creation, so approval applies to the displayed file.
fn executable(path: &Path, approve: impl FnOnce(&Path, i32) -> bool) -> io::Result<Option<Child>> {
    let path = path.canonicalize()?;
    let file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ.0)
        .open(&path)?;
    let status = trusted(&path, &file);
    if status != 0 && !approve(&path, status) {
        return Ok(None);
    }
    let result = Command::new(&path)
        .current_dir(path.parent().unwrap_or(Path::new(".")))
        .spawn();
    drop(file);
    result.map(Some)
}

pub(super) fn open(owner: HWND, path: &Path) -> Result<(), String> {
    let is_program = path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("exe") || ext.eq_ignore_ascii_case("com"));
    if is_program {
        match executable(path, |path, status| unsafe {
            MessageBoxW(
                Some(owner),
                &HSTRING::from(format!(
                    "署名がないか、署名の有効性を確認できないアプリです。\n\n{}\n\n確認結果: 0x{:08X}\n信頼できるアプリの場合に限り、このファイルの実行を承認しますか？",
                    path.display(),
                    status as u32
                )),
                w!("アプリの実行を承認"),
                MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2,
            ) == IDYES
        }) {
            Ok(_) => return Ok(()), // The launched application belongs to the user.
            Err(e) if e.raw_os_error() == Some(740) => return shell(owner, path, w!("runas")),
            Err(e) => {
                return Err(format!(
                    "{} を起動できませんでした。\n{e}\nWindowsのアプリ制御・管理ポリシーによる拒否は、この承認では解除されません。",
                    path.display()
                ));
            }
        }
    }
    shell(owner, path, w!("open"))
}

fn shell(owner: HWND, path: &Path, verb: PCWSTR) -> Result<(), String> {
    unsafe {
        let name = HSTRING::from(path.as_os_str());
        let folder = HSTRING::from(path.parent().unwrap_or(Path::new(".")).as_os_str());
        let mut info = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOASYNC,
            hwnd: owner,
            lpVerb: verb,
            lpFile: PCWSTR(name.as_ptr()),
            lpDirectory: PCWSTR(folder.as_ptr()),
            nShow: SW_SHOWNORMAL.0,
            ..Default::default()
        };
        ShellExecuteExW(&mut info)
            .map_err(|e| format!("{} を開けませんでした。\n{e}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unsigned_application_needs_consent_and_runs_only_after_approval() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("helper.rs");
        let program = dir.path().join("署名なし アプリ.exe");
        let marker = dir.path().join("started");
        std::fs::write(&source, format!("#![windows_subsystem=\"windows\"]\nfn main() {{ std::fs::write({:?}, b\"approved\").unwrap(); }}", marker.to_str().unwrap())).unwrap();
        let output = Command::new("rustc")
            .arg(&source)
            .args(["--crate-name", "consent_helper", "-o"])
            .arg(&program)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let denied = executable(&program, |selected, status| {
            assert_eq!(selected, program.canonicalize().unwrap());
            assert_ne!(status, 0);
            false
        })
        .unwrap();
        assert!(denied.is_none() && !marker.exists());
        let child = executable(&program, |selected, status| {
            assert_ne!(status, 0);
            // An approval must not be substitutable by rewriting the executable.
            assert!(OpenOptions::new().write(true).open(selected).is_err());
            assert!(std::fs::remove_file(selected).is_err());
            true
        })
        .unwrap()
        .unwrap();
        struct Worker(Child);
        impl Drop for Worker {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let mut worker = Worker(child);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if let Some(exit) = worker.0.try_wait().unwrap() {
                assert!(exit.success());
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "approved helper did not exit"
            );
            std::thread::park_timeout(std::time::Duration::from_millis(10));
        }
        assert_eq!(std::fs::read(marker).unwrap(), b"approved");
    }
}
