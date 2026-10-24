//! Read-only SSH inputs using the same settings, resolver and Rust client as yyeditor.
use crate::{Cancellation, Result};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use yy_remote::{Connector, ConnectorOptions, Prompter, RemoteFs, RemoteUri, SftpFs};

pub use yy_remote::Prompter as InputPrompter;
pub use yy_remote::{HostKeyCheck, HostKeyQuestion};

pub struct Download {
    pub path: PathBuf,
    pub label: String,
    _directory: tempfile::TempDir,
}

pub fn resolver(config: &yy_config::RemoteConfig) -> yy_remote::ssh_config::Resolver {
    use yy_remote::ssh_config::{HostOverride, Resolver};
    let mut resolver = Resolver::from_environment(config.read_ssh_config);
    resolver.proxy = (!config.proxy.is_empty()).then(|| config.proxy.clone());
    resolver.hosts = config
        .host
        .iter()
        .map(|(name, h)| {
            (
                name.clone(),
                HostOverride {
                    ssh: h.ssh.clone(),
                    hostname: h.hostname.clone(),
                    user: h.user.clone(),
                    port: h.port,
                    identity_file: h.identity_file.clone(),
                    proxy_jump: h.proxy_jump.clone(),
                    jump_command: h.jump_command.clone(),
                    sftp_subsystem: h.sftp_subsystem.clone(),
                    sftp_server: h.sftp_server.clone(),
                    proxy: h.proxy.clone(),
                    agent_dir: h.agent_dir.clone(),
                    forward: h.forward.clone(),
                },
            )
        })
        .collect();
    resolver
}

pub fn download(
    path: &Path,
    prompter: &dyn Prompter,
    cancel: &Cancellation,
) -> Result<Option<Download>> {
    download_with_progress(path, prompter, cancel, &|_| {})
}

pub fn download_with_progress(
    path: &Path,
    prompter: &dyn Prompter,
    cancel: &Cancellation,
    progress: &dyn Fn(u64),
) -> Result<Option<Download>> {
    let Some(text) = path.to_str().filter(|s| s.starts_with("ssh:")) else {
        return Ok(None);
    };
    let uri = RemoteUri::parse(text)
        .ok_or("invalid SSH URI; use ssh://[user@]host[:port]/absolute/path")?;
    if uri.path.contains(&0) {
        return Err("SSH path cannot contain NUL".into());
    }
    cancel.check()?;
    let (config, error) = yy_config::Config::load();
    if let Some(error) = error {
        return Err(format!("SSH configuration: {error}"));
    }
    let dir = yy_config::config_dir().ok_or("configuration directory unavailable")?;
    let config = config.remote;
    let options = ConnectorOptions {
        known_hosts: dir.join("known_hosts"),
        extra_known_hosts: if config.read_ssh_known_hosts {
            std::env::home_dir()
                .map(|h| vec![h.join(".ssh/known_hosts")])
                .unwrap_or_default()
        } else {
            vec![]
        },
        keepalive: Duration::from_secs(u64::from(config.keepalive_secs.max(1))),
        passwords: None,
    };
    let spec = resolver(&config).resolve(&uri.target());
    let transport = yy_ssh::SshConnector::new(&options)
        .connect(&spec, prompter, &yy_remote::ConnectLog::new())
        .map_err(|e| e.to_string())?;
    cancel.check()?;
    let fs = Arc::new(SftpFs::connect(transport.as_ref()).map_err(|e| e.to_string())?);
    fetch_progress(fs, &uri, cancel, progress).map(Some)
}

pub fn fetch(fs: Arc<SftpFs>, uri: &RemoteUri, cancel: &Cancellation) -> Result<Download> {
    fetch_progress(fs, uri, cancel, &|_| {})
}

fn fetch_progress(
    fs: Arc<SftpFs>,
    uri: &RemoteUri,
    cancel: &Cancellation,
    progress: &dyn Fn(u64),
) -> Result<Download> {
    use yy_remote::transfer::{Loc, copy};
    let before = fs.stat(&uri.path).map_err(|e| e.to_string())?;
    if before.kind != yy_remote::FileKind::File {
        return Err("SSH input must be a regular file".into());
    }
    let directory = tempfile::Builder::new()
        .prefix("yymerge-ssh-")
        .tempdir()
        .map_err(|e| e.to_string())?;
    // Retain the suffix for Office/image auto-detection, never remote directory names.
    let extension = Path::new(std::str::from_utf8(&uri.path).unwrap_or(""))
        .extension()
        .and_then(|s| s.to_str())
        .filter(|s| s.len() <= 12 && s.chars().all(|c| c.is_ascii_alphanumeric()));
    let path = directory
        .path()
        .join(format!("input.{}", extension.unwrap_or("txt")));
    let stats = copy(
        &Loc::Sftp(fs.clone(), uri.path.clone()),
        &Loc::Local(path.clone()),
        &mut |stats| {
            progress(stats.bytes);
            cancel.check().is_ok()
        },
    )
    .map_err(|e| {
        if cancel.check().is_err() {
            "cancelled".into()
        } else {
            e.to_string()
        }
    })?;
    cancel.check()?;
    let after = fs.stat(&uri.path).map_err(|e| e.to_string())?;
    if before.id != after.id || stats.bytes != before.len() {
        return Err("remote file changed during download; retry the comparison".into());
    }
    Ok(Download {
        path,
        label: uri.to_string(),
        _directory: directory,
    })
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    #[test]
    fn sftp_large_input_transfer_cleanup_cancel_and_source_changes() {
        use super::*;
        use yy_remote::local::LocalTransport;
        let Some(_) = yy_remote::local::sftp_server() else {
            assert!(
                std::env::var_os("YYMERGE_REQUIRE_SFTP").is_none(),
                "SFTP server required"
            );
            eprintln!("SFTP transport test requires openssh-sftp-server");
            return;
        };
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("日本語 file.dat");
        let file = std::fs::File::create(&source).unwrap();
        file.set_len(33 * 1024 * 1024).unwrap();
        drop(file);
        let fs = Arc::new(SftpFs::connect(&LocalTransport::new()).unwrap());
        let uri = RemoteUri {
            user: None,
            host: "test".into(),
            port: None,
            path: source.to_str().unwrap().as_bytes().to_vec(),
        };
        let downloaded = fetch(fs.clone(), &uri, &Cancellation::new()).unwrap();
        assert_eq!(
            std::fs::metadata(&downloaded.path).unwrap().len(),
            33 * 1024 * 1024
        );
        let path = downloaded.path.clone();
        drop(downloaded);
        assert!(!path.exists());
        let cancel = Cancellation::new();
        cancel.cancel();
        assert!(fetch(fs.clone(), &uri, &cancel).is_err());
        let changed = std::sync::atomic::AtomicBool::new(false);
        let result = fetch_progress(fs, &uri, &Cancellation::new(), &|count| {
            if count > 0 && !changed.swap(true, std::sync::atomic::Ordering::Relaxed) {
                std::fs::OpenOptions::new()
                    .write(true)
                    .open(&source)
                    .unwrap()
                    .set_len(34 * 1024 * 1024)
                    .unwrap();
            }
        });
        assert!(result.is_err());
    }
    #[test]
    fn shared_profiles_preserve_gateway_settings() {
        let config = yy_config::Config::from_toml("[remote.host.demo]\nhostname='127.0.0.1'\nuser='tester'\nport=2222\nsftp_server='/usr/lib/openssh/sftp-server'\n").unwrap();
        let spec = super::resolver(&config.remote)
            .resolve(&yy_remote::uri::Target::parse("demo").unwrap());
        assert_eq!(spec.hostname, "127.0.0.1");
        assert_eq!(spec.port, 2222);
        assert_eq!(spec.user, "tester");
        assert!(matches!(
            spec.sftp,
            yy_remote::ssh_config::SftpStart::Command(_)
        ));
    }
}
