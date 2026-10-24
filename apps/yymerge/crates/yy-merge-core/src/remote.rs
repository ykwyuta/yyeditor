//! Read-only SSH inputs using the same settings, resolver and Rust client as yyeditor.
use crate::{Cancellation, Result};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use yy_remote::{Connector, ConnectorOptions, Prompter, RemoteUri, SftpFs};

pub use yy_remote::Prompter as InputPrompter;
pub use yy_remote::{HostKeyCheck, HostKeyQuestion};

pub struct Download {
    pub path: PathBuf,
    pub label: String,
    _directory: tempfile::TempDir,
}

pub fn resolver(config: &yy_config::RemoteConfig) -> yy_remote::ssh_config::Resolver {
    yy_remote::policy::resolver(config)
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
    use yy_remote::transfer::Loc;
    let source = if yy_remote::policy::use_agent(&config, &uri.target(), false) {
        let session = yy_remote::Session::start(
            transport,
            &yy_remote::AgentFiles::beside_exe(),
            spec.agent_dir.as_deref(),
            &yy_remote::ConnectLog::new(),
        )
        .map_err(|e| e.to_string())?;
        Loc::Remote(Arc::new(session), uri.path.clone())
    } else {
        let protocol =
            yy_remote::policy::protocol(&config, &uri.target(), yy_remote::xfer::Protocol::Sftp);
        let listing =
            yy_remote::policy::listing(&config, &uri.target()).map_err(|e| e.to_string())?;
        let access = yy_remote::file_access::FileAccess::connect(transport, protocol)
            .map_err(|e| e.to_string())?
            .with_listing(listing);
        Loc::Native(Arc::new(access), uri.path.clone())
    };
    fetch_location(source, &uri, cancel, progress).map(Some)
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
    fetch_location(
        yy_remote::transfer::Loc::Sftp(fs, uri.path.clone()),
        uri,
        cancel,
        progress,
    )
}

fn fetch_location(
    source: yy_remote::transfer::Loc,
    uri: &RemoteUri,
    cancel: &Cancellation,
    progress: &dyn Fn(u64),
) -> Result<Download> {
    use yy_remote::transfer::{Loc, copy, info};
    let before = info(&source)
        .map_err(|e| e.to_string())?
        .ok_or("remote file not found")?;
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
    let stats = copy(&source, &Loc::Local(path.clone()), &mut |stats| {
        progress(stats.bytes);
        cancel.check().is_ok()
    })
    .map_err(|e| {
        if cancel.check().is_err() {
            "cancelled".into()
        } else {
            e.to_string()
        }
    })?;
    cancel.check()?;
    let after = info(&source)
        .map_err(|e| e.to_string())?
        .ok_or("remote file was removed")?;
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
    #[cfg(unix)]
    #[test]
    fn scp_comparison_input_never_starts_sftp() {
        use super::*;
        use yy_remote::{Process, Transport};
        struct ScpOnly(yy_remote::local::LocalTransport);
        impl Transport for ScpOnly {
            fn exec(&self, command: &[u8]) -> std::io::Result<Process> {
                self.0.exec(command)
            }
            fn subsystem(&self, _: &str) -> std::io::Result<Process> {
                panic!("SCP input must not request SFTP")
            }
            fn is_closed(&self) -> bool {
                false
            }
        }
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("日本語 file.csv");
        std::fs::write(&source, b"one,two\n").unwrap();
        let uri = RemoteUri {
            user: None,
            host: "scp".into(),
            port: None,
            path: source.to_str().unwrap().as_bytes().to_vec(),
        };
        let access = yy_remote::file_access::FileAccess::connect(
            Arc::new(ScpOnly(yy_remote::local::LocalTransport::new())),
            yy_remote::xfer::Protocol::Scp,
        )
        .unwrap();
        let location = yy_remote::transfer::Loc::Native(Arc::new(access), uri.path.clone());
        let got = fetch_location(location.clone(), &uri, &Cancellation::new(), &|_| {}).unwrap();
        assert_eq!(std::fs::read(&got.path).unwrap(), b"one,two\n");
        assert_eq!(got.path.extension().unwrap(), "csv");
        let temp = got.path.clone();
        drop(got);
        assert!(!temp.exists());
        let stop = Cancellation::new();
        stop.cancel();
        assert!(fetch_location(location, &uri, &stop, &|_| {}).is_err());
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
