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
    #[serde(default)]
    pub native: bool,
    pub driver: String,
    pub server: String,
    pub port: String,
    pub database: String,
    pub username: String,
    pub password: String,
    pub options: String,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum StoredProfiles {
    Current {
        #[serde(default)]
        version: u32,
        profiles: Vec<Profile>,
    },
    Legacy(Vec<Profile>),
}

fn embedded_demo_profiles(path: &Path) -> Result<Vec<Profile>> {
    let directory = path
        .parent()
        .ok_or("接続設定の保存先が不正です")?
        .join("demo");
    std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    let directory = directory.canonicalize().map_err(|e| e.to_string())?;
    [
        (
            DbKind::Sqlite,
            "SQLite",
            "demo.sqlite",
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../../../demo/yydbmanager/data/demo.sqlite"
            ))
            .as_slice(),
        ),
        (
            DbKind::DuckDb,
            "DuckDB",
            "demo.duckdb",
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../../../demo/yydbmanager/data/demo.duckdb"
            ))
            .as_slice(),
        ),
    ]
    .into_iter()
    .map(|(kind, name, filename, data)| {
        let target = directory.join(filename);
        if !target.exists() {
            let mut file =
                tempfile::NamedTempFile::new_in(&directory).map_err(|e| e.to_string())?;
            file.write_all(data)
                .and_then(|_| file.as_file().sync_all())
                .map_err(|e| e.to_string())?;
            if let Err(e) = file.persist_noclobber(&target)
                && e.error.kind() != std::io::ErrorKind::AlreadyExists
            {
                return Err(e.to_string());
            }
        }
        Ok(Profile {
            name: format!("demo / {name}"),
            kind,
            target: target.to_string_lossy().into_owned(),
            details: None,
        })
    })
    .collect()
}

fn demo_profiles(path: &Path) -> Result<Vec<Profile>> {
    let mut profiles: Vec<Profile> = [
        (DbKind::PostgreSql, "PostgreSQL", 25432, "yydemo", "yy_demo"),
        (DbKind::MySql, "MySQL", 23306, "yydemo", "yy_demo"),
        (DbKind::SqlServer, "SQL Server", 21433, "yydemo", "sa"),
        (DbKind::Oracle, "Oracle", 21521, "FREEPDB1", "yy_demo"),
        (DbKind::Db2, "Db2", 25000, "YYDEMO", "db2inst1"),
    ]
    .into_iter()
    .map(|(kind, name, port, database, username)| {
        let config = yy_db_core::native::Config {
            host: "127.0.0.1".into(),
            port,
            database: database.into(),
            username: username.into(),
            password: "YyDemo_2026!".into(),
            tls: "disable".into(),
            ca_file: None,
            trust_server_certificate: false,
        };
        Ok(Profile {
            name: format!("demo / {name}"),
            kind,
            target: config.encode()?,
            details: Some(ConnectionDetails {
                native: true,
                server: config.host,
                port: port.to_string(),
                database: config.database,
                username: config.username,
                password: config.password,
                options: r#"{"tls":"disable"}"#.into(),
                ..Default::default()
            }),
        })
    })
    .collect::<Result<_>>()?;
    profiles.extend(embedded_demo_profiles(path)?);
    Ok(profiles)
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
        let profiles = demo_profiles(path)?;
        save(path, &profiles)?;
        return Ok(profiles);
    }
    if std::fs::metadata(path).map_err(|e| e.to_string())?.len() > 4 * 1024 * 1024 {
        return Err("接続設定が大きすぎます".into());
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let stored: StoredProfiles =
        serde_json::from_slice(&crypt(&bytes, false)?).map_err(|e| e.to_string())?;
    match stored {
        StoredProfiles::Current {
            version,
            mut profiles,
        } => {
            if version < 2 {
                for demo in embedded_demo_profiles(path)? {
                    if !profiles.iter().any(|p| p.name == demo.name) {
                        profiles.push(demo);
                    }
                }
                save(path, &profiles)?;
            }
            Ok(profiles)
        }
        StoredProfiles::Legacy(mut profiles) => {
            for demo in demo_profiles(path)? {
                if !profiles.iter().any(|p| p.name == demo.name) {
                    profiles.push(demo);
                }
            }
            save(path, &profiles)?;
            Ok(profiles)
        }
    }
}
pub fn save(path: &Path, profiles: &[Profile]) -> Result<()> {
    let parent = path.parent().ok_or("接続設定の保存先が不正です")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let bytes = crypt(
        &serde_json::to_vec(&serde_json::json!({"version": 2, "profiles": profiles}))
            .map_err(|e| e.to_string())?,
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
    fn version_one_adds_only_embedded_demos_and_preserves_user_data() {
        use std::sync::atomic::AtomicBool;
        use yy_db_core::{Database, Options};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("connections.dat");
        let bytes = crypt(br#"{"version":1,"profiles":[]}"#, true).unwrap();
        std::fs::write(&path, bytes).unwrap();
        let profiles = load(&path).unwrap();
        assert_eq!(profiles.len(), 2);
        for profile in &profiles {
            assert!(Path::new(&profile.target).is_absolute());
            let mut options = Options::memory(profile.kind);
            options.path = Some(profile.target.clone().into());
            let mut db = Database::open(&options).unwrap();
            let results = db
                .execute("SELECT COUNT(*) FROM orders", &AtomicBool::new(false))
                .unwrap();
            assert_eq!(results[0].page(0, 1).unwrap()[0][0].display(), "1200");
            db.execute(
                "CREATE TABLE user_demo_marker (id INTEGER)",
                &AtomicBool::new(false),
            )
            .unwrap();
        }
        // Initialization must not overwrite an existing, edited demo database.
        embedded_demo_profiles(&path).unwrap();
        for profile in &profiles {
            let mut options = Options::memory(profile.kind);
            options.path = Some(profile.target.clone().into());
            let mut db = Database::open(&options).unwrap();
            db.execute("SELECT * FROM user_demo_marker", &AtomicBool::new(false))
                .unwrap();
        }
        save(&path, &[]).unwrap();
        assert!(load(&path).unwrap().is_empty());
    }

    #[test]
    fn demos_are_seeded_once_and_remain_editable_and_deletable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("connections.dat");
        let mut profiles = load(&path).unwrap();
        assert_eq!(profiles.len(), 7);
        for profile in &profiles[..5] {
            let config = yy_db_core::native::Config::decode(&profile.target)
                .unwrap()
                .unwrap();
            config.validate().unwrap();
            assert_eq!(config.host, "127.0.0.1");
            assert_eq!(config.tls, "disable");
            assert!(profile.details.as_ref().unwrap().native);
        }
        profiles[0].name = "自分の接続".into();
        profiles.remove(1);
        save(&path, &profiles).unwrap();
        let restored = load(&path).unwrap();
        assert_eq!(restored.len(), 6);
        assert_eq!(restored[0].name, "自分の接続");
        save(&path, &[]).unwrap();
        assert!(load(&path).unwrap().is_empty());
    }

    #[test]
    fn legacy_settings_keep_existing_connections_and_seed_only_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("connections.dat");
        let mut profiles = demo_profiles(&path).unwrap();
        profiles.truncate(1);
        profiles[0].target = "existing-user-settings".into();
        let bytes = crypt(&serde_json::to_vec(&profiles).unwrap(), true).unwrap();
        std::fs::write(&path, bytes).unwrap();
        let migrated = load(&path).unwrap();
        assert_eq!(migrated.len(), 7);
        assert_eq!(migrated[0].target, "existing-user-settings");
        assert_eq!(load(&path).unwrap().len(), 7);
    }

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
