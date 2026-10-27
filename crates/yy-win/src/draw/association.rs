//! Per-user Open With registration; an existing default handler is not replaced.
use windows::{
    Win32::System::Registry::*,
    core::{HSTRING, PCWSTR},
};
pub fn entries(exe: &str) -> Vec<(String, String, String)> {
    vec![
        (
            "Software\\Classes\\YYDraw.Document".into(),
            "".into(),
            "yydraw 構成図".into(),
        ),
        (
            "Software\\Classes\\YYDraw.Document\\DefaultIcon".into(),
            "".into(),
            format!("\"{exe}\",0"),
        ),
        (
            "Software\\Classes\\YYDraw.Document\\shell\\open\\command".into(),
            "".into(),
            format!("\"{exe}\" \"%1\""),
        ),
    ]
}
pub fn register() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe = exe.to_str().ok_or("実行ファイルのパスを取得できません")?;
    for (path, name, value) in entries(exe) {
        write(
            &path,
            &name,
            REG_SZ,
            &value
                .encode_utf16()
                .chain(Some(0))
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>(),
        )?;
    }
    write(
        "Software\\Classes\\.yyd\\OpenWithProgids",
        "YYDraw.Document",
        REG_NONE,
        &[],
    )?;
    unsafe {
        let key = HSTRING::from("Software\\Classes\\.yyd");
        let status = RegGetValueW(
            HKEY_CURRENT_USER,
            &key,
            PCWSTR::null(),
            RRF_RT_REG_SZ,
            None,
            None,
            None,
        );
        if status == windows::Win32::Foundation::ERROR_FILE_NOT_FOUND {
            write(
                "Software\\Classes\\.yyd",
                "",
                REG_SZ,
                &"YYDraw.Document"
                    .encode_utf16()
                    .chain(Some(0))
                    .flat_map(u16::to_le_bytes)
                    .collect::<Vec<_>>(),
            )?;
        }
        windows::Win32::UI::Shell::SHChangeNotify(
            windows::Win32::UI::Shell::SHCNE_ASSOCCHANGED,
            windows::Win32::UI::Shell::SHCNF_IDLIST,
            None,
            None,
        );
    }
    Ok(())
}
fn write(path: &str, name: &str, kind: REG_VALUE_TYPE, bytes: &[u8]) -> Result<(), String> {
    unsafe {
        let mut key = HKEY::default();
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
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
        .map_err(|e| e.to_string())?;
        let result = RegSetValueExW(key, &HSTRING::from(name), None, kind, Some(bytes))
            .ok()
            .map_err(|e| e.to_string());
        let _ = RegCloseKey(key);
        result
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn registration_quotes_executable_and_argument() {
        let entries = super::entries(r"C:\Program Files\日本語\yydraw.exe");
        assert_eq!(
            entries[2].2,
            "\"C:\\Program Files\\日本語\\yydraw.exe\" \"%1\""
        );
        assert!(
            entries
                .iter()
                .all(|(key, _, _)| key.starts_with("Software\\Classes\\YYDraw.Document"))
        );
    }
}
