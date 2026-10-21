use serde::{Deserialize, Serialize};
use std::{io::Write, path::Path};
use windows::Win32::{Foundation::LocalFree, Security::Cryptography::*};
use yy_db_core::{DbKind, Result};

#[derive(Clone, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    pub kind: DbKind,
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<ConnectionDetails>,
}
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct ConnectionDetails {
    pub driver: String,
    pub server: String,
    pub port: String,
    pub database: String,
    pub username: String,
    pub password: String,
    pub options: String,
}

fn crypt(bytes: &[u8], protect: bool) -> Result<Vec<u8>> {
    let input = CRYPT_INTEGER_BLOB {
        cbData: bytes.len().try_into().map_err(|_| "設定が大きすぎます")?,
        pbData: bytes.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        let result = if protect {
            CryptProtectData(
                &input,
                None,
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &input,
                None,
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        };
        result.map_err(|e| format!("接続設定の暗号化／復号に失敗しました: {e}"))?;
        let value = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        let _ = LocalFree(Some(windows::Win32::Foundation::HLOCAL(
            output.pbData.cast(),
        )));
        Ok(value)
    }
}
pub fn load(path: &Path) -> Result<Vec<Profile>> {
    if !path.exists() {
        return Ok(vec![]);
    }
    if std::fs::metadata(path).map_err(|e| e.to_string())?.len() > 4 * 1024 * 1024 {
        return Err("接続設定が大きすぎます".into());
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    serde_json::from_slice(&crypt(&bytes, false)?).map_err(|e| e.to_string())
}
pub fn save(path: &Path, profiles: &[Profile]) -> Result<()> {
    let parent = path.parent().ok_or("接続設定の保存先が不正です")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let bytes = crypt(
        &serde_json::to_vec(profiles).map_err(|e| e.to_string())?,
        true,
    )?;
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    file.write_all(&bytes)
        .and_then(|_| file.as_file().sync_all())
        .map_err(|e| e.to_string())?;
    file.persist(path).map_err(|e| e.to_string())?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn profiles_are_encrypted_and_replaceable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("connections.dat");
        let mut profiles = vec![Profile {
            name: "開発DB".into(),
            kind: DbKind::PostgreSql,
            target: "Pwd=test-secret;Uid=developer;".into(),
            details: None,
        }];
        save(&path, &profiles).unwrap();
        assert!(
            !std::fs::read(&path)
                .unwrap()
                .windows(11)
                .any(|v| v == b"test-secret")
        );
        assert_eq!(load(&path).unwrap()[0].target, profiles[0].target);
        profiles[0].name = "検証DB".into();
        save(&path, &profiles).unwrap();
        assert_eq!(load(&path).unwrap()[0].name, "検証DB");
    }
}
