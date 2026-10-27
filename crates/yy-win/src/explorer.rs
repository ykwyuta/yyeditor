//! Per-user Explorer verbs. Only our own keys are created or removed.
use std::path::Path;
use windows::{
    Win32::{Foundation::*, System::Registry::*, UI::Shell::*},
    core::{HSTRING, PCWSTR},
};

pub(crate) const ID_REGISTER: u16 = 62001;
pub(crate) const ID_UNREGISTER: u16 = 62002;

#[derive(Clone, Copy)]
pub enum ExplorerApp {
    Editor,
    Terminal,
}

impl ExplorerApp {
    fn name(self) -> &'static str {
        match self {
            Self::Editor => "yyeditor",
            Self::Terminal => "yyterm",
        }
    }

    fn keys(self) -> Vec<(String, &'static str)> {
        let mut targets = vec![
            ("Directory", "%1\\."),
            ("Directory\\Background", "%V\\."),
            ("Drive", "%1\\."),
        ];
        if matches!(self, Self::Editor) {
            targets.push(("*", "%1"));
        }
        targets
            .into_iter()
            .map(|(target, arg)| {
                (
                    format!("Software\\Classes\\{target}\\shell\\{}.Open", self.name()),
                    arg,
                )
            })
            .collect()
    }

    fn entries(self, exe: &Path) -> Vec<(String, String, String)> {
        let exe = exe.to_string_lossy();
        self.keys()
            .into_iter()
            .flat_map(|(key, arg)| {
                vec![
                    (key.clone(), "".into(), format!("{} で開く", self.name())),
                    (key.clone(), "Icon".into(), format!("\"{exe}\",0")),
                    (key.clone(), "MultiSelectModel".into(), "Single".into()),
                    (
                        format!("{key}\\command"),
                        "".into(),
                        format!("\"{exe}\" \"{arg}\""),
                    ),
                ]
            })
            .collect()
    }
}

fn write(root: HKEY, path: &str, name: &str, value: &str) -> Result<(), String> {
    unsafe {
        let mut key = HKEY::default();
        RegCreateKeyExW(
            root,
            &HSTRING::from(path),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut key,
            None,
        )
        .ok()
        .map_err(|e| format!("{path}: {e}"))?;
        let bytes: Vec<_> = value
            .encode_utf16()
            .chain(Some(0))
            .flat_map(u16::to_le_bytes)
            .collect();
        let result = RegSetValueExW(key, &HSTRING::from(name), None, REG_SZ, Some(&bytes)).ok();
        let _ = RegCloseKey(key);
        result.map_err(|e| format!("{path}: {e}"))
    }
}

fn update(app: ExplorerApp, register: bool) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    apply(HKEY_CURRENT_USER, app, register, &exe)?;
    unsafe {
        SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None);
    }
    Ok(())
}

fn apply(root: HKEY, app: ExplorerApp, register: bool, exe: &Path) -> Result<(), String> {
    if register {
        for (path, name, value) in app.entries(exe) {
            write(root, &path, &name, &value)?;
        }
    } else {
        for (key, _) in app.keys() {
            let status = unsafe { RegDeleteTreeW(root, &HSTRING::from(&key)) };
            if status != ERROR_FILE_NOT_FOUND && status != ERROR_PATH_NOT_FOUND {
                status.ok().map_err(|e| format!("{key}: {e}"))?;
            }
        }
    }
    Ok(())
}

pub(crate) fn menu(hwnd: HWND, app: ExplorerApp, register: bool) {
    match update(app, register) {
        Ok(()) => crate::util::info_box(
            hwnd,
            if register {
                "エクスプローラーの右クリックメニューに登録しました。\nWindows 11では「その他のオプションを確認」内に表示されます。\n実行ファイルを移動した場合は、移動先から登録し直してください。"
            } else {
                "エクスプローラーの右クリックメニューから解除しました。"
            },
        ),
        Err(e) => crate::util::error_box(
            hwnd,
            &format!("右クリックメニューを変更できませんでした。\n{e}"),
        ),
    }
}

/// Handle explicit registration switches before starting the GUI (0 success / 1 error).
pub fn explorer_command(app: ExplorerApp) -> Option<i32> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 1 {
        return None;
    }
    let register = if args[0] == "--register-context-menu" {
        true
    } else if args[0] == "--unregister-context-menu" {
        false
    } else {
        return None;
    };
    Some(match update(app, register) {
        Ok(()) => 0,
        Err(e) => {
            crate::util::error_box(
                HWND::default(),
                &format!("右クリックメニューを変更できませんでした。\n{e}"),
            );
            1
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestHive(HKEY, String);
    impl Drop for TestHive {
        fn drop(&mut self) {
            unsafe {
                let _ = RegCloseKey(self.0);
                let _ = RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(&self.1));
            }
        }
    }

    #[test]
    fn registration_round_trip_preserves_other_verbs_and_app() {
        let path = format!(
            "Software\\YYEditor.ExplorerTest.{}.{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let mut root = HKEY::default();
        unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                &HSTRING::from(&path),
                None,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_ALL_ACCESS,
                None,
                &mut root,
                None,
            )
            .ok()
            .unwrap();
        }
        let hive = TestHive(root, path);
        let unrelated = "Software\\Classes\\Directory\\shell\\Other.App\\command";
        write(hive.0, unrelated, "", "leave this alone").unwrap();
        let editor = ExplorerApp::Editor;
        let term = ExplorerApp::Terminal;
        let exe = Path::new(r"C:\日本語 & space\yy.exe");
        apply(hive.0, editor, true, exe).unwrap();
        apply(hive.0, editor, true, exe).unwrap();
        apply(hive.0, term, true, exe).unwrap();
        for (path, name, value) in editor.entries(exe).into_iter().chain(term.entries(exe)) {
            assert_eq!(read(hive.0, &path, &name).unwrap(), value);
        }
        apply(hive.0, editor, false, exe).unwrap();
        apply(hive.0, editor, false, exe).unwrap();
        for (path, _) in editor.keys() {
            assert!(read(hive.0, &path, "").is_err());
        }
        for (path, name, value) in term.entries(exe) {
            assert_eq!(read(hive.0, &path, &name).unwrap(), value);
        }
        apply(hive.0, term, false, exe).unwrap();
        assert_eq!(read(hive.0, unrelated, "").unwrap(), "leave this alone");
    }

    fn read(root: HKEY, path: &str, name: &str) -> Result<String, String> {
        let mut buf = [0u16; 1024];
        let mut bytes = std::mem::size_of_val(&buf) as u32;
        unsafe {
            RegGetValueW(
                root,
                &HSTRING::from(path),
                &HSTRING::from(name),
                RRF_RT_REG_SZ,
                None,
                Some(buf.as_mut_ptr().cast()),
                Some(&mut bytes),
            )
            .ok()
            .map_err(|e| e.to_string())?;
        }
        Ok(String::from_utf16_lossy(&buf[..bytes as usize / 2 - 1]))
    }

    #[test]
    fn verbs_are_per_user_quoted_and_scoped_to_each_app() {
        let exe = Path::new(r"C:\Program Files\日本語 & tools\yyeditor.exe");
        let editor = ExplorerApp::Editor.entries(exe);
        assert_eq!(editor.len(), 16);
        assert!(
            editor
                .iter()
                .all(|(key, _, _)| key.starts_with("Software\\Classes\\")
                    && key.contains("\\shell\\yyeditor.Open"))
        );
        assert!(editor.iter().any(|(key, _, value)| key.ends_with("command")
            && value == "\"C:\\Program Files\\日本語 & tools\\yyeditor.exe\" \"%1\""));
        let term = ExplorerApp::Terminal.entries(Path::new(r"C:\YY\yyterm.exe"));
        assert_eq!(term.len(), 12);
        assert!(
            term.iter()
                .all(|(key, _, _)| !key.contains("\\*\\") && key.contains("\\shell\\yyterm.Open"))
        );
        assert!(
            term.iter()
                .any(|(key, _, value)| key.contains("Background") && value.ends_with("\"%V\\.\""))
        );
        assert!(
            term.iter()
                .any(|(key, _, value)| key.contains("Drive") && value.ends_with("\"%1\\.\""))
        );
    }

    #[test]
    fn windows_arguments_keep_drive_roots_unc_and_shell_characters_intact() {
        let exe = Path::new(r"C:\Program Files\日本語 & tools\yyterm.exe");
        let template = ExplorerApp::Terminal
            .entries(exe)
            .into_iter()
            .find(|(key, _, _)| key.contains("Background") && key.ends_with("command"))
            .unwrap()
            .2;
        for folder in [r"C:\", r"C:\日本語 & $(literal)\folder", r"\\server\share\"] {
            let command = HSTRING::from(template.replace("%V", folder));
            let mut argc = 0;
            unsafe {
                let argv = CommandLineToArgvW(&command, &mut argc);
                assert!(!argv.is_null());
                let args: Vec<_> = std::slice::from_raw_parts(argv, argc as usize)
                    .iter()
                    .map(|arg| arg.to_string().unwrap())
                    .collect();
                let _ = LocalFree(Some(HLOCAL(argv.cast())));
                assert_eq!(
                    args,
                    vec![exe.to_string_lossy().into_owned(), format!("{folder}\\.")]
                );
            }
        }
    }
}
