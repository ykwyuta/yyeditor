//! GUI-managed connection profiles, separate from editor preferences and OpenSSH config.
use crate::{ConfigError, RemoteConfig, RemoteHost};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Profiles {
    pub version: u32,
    pub hosts: BTreeMap<String, RemoteHost>,
    pub removed: Vec<String>,
}
impl Default for Profiles {
    fn default() -> Self {
        Self {
            version: 1,
            hosts: BTreeMap::new(),
            removed: Vec::new(),
        }
    }
}

pub fn beside(config: &Path) -> PathBuf {
    config.with_file_name("connections.toml")
}
pub fn default_path() -> Option<PathBuf> {
    Some(crate::config_dir()?.join("connections.toml"))
}

impl Profiles {
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(source) => {
                return Err(ConfigError::Io {
                    path: path.into(),
                    source,
                });
            }
        };
        let profiles: Self = toml::from_str(&text).map_err(|source| ConfigError::Parse {
            path: path.into(),
            source,
        })?;
        profiles.validate().map_err(|message| ConfigError::Io {
            path: path.into(),
            source: std::io::Error::new(std::io::ErrorKind::InvalidData, message),
        })?;
        Ok(profiles)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.version != 1 {
            return Err(format!("未対応の接続設定バージョン: {}", self.version));
        }
        let mut names = Vec::<String>::new();
        for (name, host) in &self.hosts {
            if name.trim().is_empty()
                || name != name.trim()
                || name.contains(['@', ':', '/', '\\'])
                || name.chars().any(|c| c.is_whitespace() || c.is_control())
            {
                return Err("接続名には空白・@・:・/・\\を使用できません".into());
            }
            if names.iter().any(|n| n.eq_ignore_ascii_case(name)) {
                return Err(format!("接続名が重複しています: {name}"));
            }
            names.push(name.clone());
            validate_host(host).map_err(|e| format!("{name}: {e}"))?;
        }
        Ok(())
    }

    pub fn merge_into(&self, config: &mut RemoteConfig) {
        config
            .host
            .retain(|name, _| !self.removed.iter().any(|n| n.eq_ignore_ascii_case(name)));
        for (name, host) in &self.hosts {
            config.host.retain(|key, _| !key.eq_ignore_ascii_case(name));
            config.host.insert(name.clone(), host.clone());
        }
    }

    /// Reload before changing one profile; retain other profiles and atomically replace the file.
    pub fn edit(path: &Path, name: &str, host: Option<RemoteHost>) -> Result<Self, String> {
        let parent = path.parent().ok_or("接続設定の保存先がありません")?;
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path.with_extension("lock"))
            .map_err(|e| e.to_string())?;
        fs2::FileExt::lock_exclusive(&lock).map_err(|e| e.to_string())?;
        let mut profiles = Self::load(path).map_err(|e| e.to_string())?;
        profiles
            .hosts
            .retain(|key, _| !key.eq_ignore_ascii_case(name));
        profiles.removed.retain(|n| !n.eq_ignore_ascii_case(name));
        if let Some(host) = host {
            profiles.hosts.insert(name.into(), host);
        } else {
            profiles.removed.push(name.into());
        }
        profiles.validate()?;
        let body = toml::to_string_pretty(&profiles).map_err(|e| e.to_string())?;
        let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
        temp.write_all(body.as_bytes())
            .and_then(|_| temp.as_file().sync_all())
            .map_err(|e| e.to_string())?;
        temp.persist(path).map_err(|e| e.to_string())?;
        Ok(profiles)
    }
}

pub fn validate_host(h: &RemoteHost) -> Result<(), String> {
    if h.ssh.connect_timeout_secs == 0 || h.ssh.connect_timeout_secs > 3600 {
        return Err("接続タイムアウトは1～3600秒です".into());
    }
    if !h.ssh.public_key_auth && !h.ssh.keyboard_interactive_auth && !h.ssh.password_auth {
        return Err("少なくとも1つの認証方式を有効にしてください".into());
    }
    if h.port == Some(0) {
        return Err("ポートは1～65535です".into());
    }
    if h.protocol.is_some() && h.use_agent == Some(true) {
        return Err("SFTP/SCP指定と専用エージェントは併用できません".into());
    }
    if h.sftp_subsystem.is_some() && h.sftp_server.is_some() {
        return Err("SFTPサブシステムとサーバーコマンドは排他です".into());
    }
    for value in [
        &h.hostname,
        &h.user,
        &h.identity_file,
        &h.proxy_jump,
        &h.jump_command,
        &h.sftp_subsystem,
        &h.sftp_server,
        &h.list_command,
        &h.proxy,
        &h.agent_dir,
        &h.remote_directory,
    ]
    .into_iter()
    .flatten()
    {
        if value.is_empty() || value.contains(['\0', '\r', '\n']) {
            return Err("設定値は空でない1行で指定してください".into());
        }
    }
    if h.list_format
        .as_deref()
        .is_some_and(|s| !matches!(s, "lines" | "nul"))
    {
        return Err("一覧形式はlinesまたはnulです".into());
    }
    if h.terminal_multiplexer != crate::TerminalMultiplexer::None
        && h.terminal_session_prefix.trim().is_empty()
    {
        return Err("tmux/screenにはセッション名プレフィクスが必須です".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn profiles_round_trip_and_override_legacy_without_touching_preferences() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config.toml");
        std::fs::write(
            &config,
            "# keep my comment\n[remote.host.LAB]\nhostname='old'\n[editor]\ntab_width=7\n",
        )
        .unwrap();
        let before = std::fs::read(&config).unwrap();
        let path = beside(&config);
        let host = RemoteHost {
            hostname: Some("gateway".into()),
            protocol: Some(crate::TransferProtocol::Scp),
            ..Default::default()
        };
        Profiles::edit(&path, "lab", Some(host.clone())).unwrap();
        Profiles::edit(&path, "other", Some(RemoteHost::default())).unwrap();
        let loaded = crate::Config::load_from(&config).unwrap();
        assert_eq!(loaded.remote.host["lab"], host);
        assert!(!loaded.remote.host.contains_key("LAB"));
        assert_eq!(loaded.remote.host.len(), 2);
        assert_eq!(std::fs::read(&config).unwrap(), before);
        let profiles = Profiles::edit(&path, "LAB", None).unwrap();
        assert!(profiles.hosts.contains_key("other"));
        assert!(!profiles.hosts.contains_key("lab"));
    }
    #[test]
    fn invalid_edit_does_not_destroy_saved_profiles() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("connections.toml");
        Profiles::edit(&path, "valid", Some(RemoteHost::default())).unwrap();
        let before = std::fs::read(&path).unwrap();
        assert!(Profiles::edit(&path, "bad name", Some(RemoteHost::default())).is_err());
        assert_eq!(std::fs::read(path).unwrap(), before);
    }

    #[test]
    fn concurrent_profile_updates_do_not_lose_other_connections() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("connections.toml");
        std::thread::scope(|scope| {
            for i in 0..12 {
                let path = &path;
                scope.spawn(move || {
                    Profiles::edit(path, &format!("site{i}"), Some(RemoteHost::default())).unwrap();
                });
            }
        });
        assert_eq!(Profiles::load(&path).unwrap().hosts.len(), 12);
    }
}
