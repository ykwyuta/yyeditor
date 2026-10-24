//! Agent-free file editing using the selected SFTP or legacy SCP protocol.
use crate::{
    FileId, FileInfo, RemoteFs, SftpFs, Transport, sftp, shellfs::ShellFs, xfer::Protocol,
};
use std::{
    io::{self, Read, Write},
    sync::Arc,
};

pub struct FileAccess {
    transport: Arc<dyn Transport>,
    fs: Arc<dyn RemoteFs>,
    sftp: Option<Arc<SftpFs>>,
}

impl FileAccess {
    pub fn connect(transport: Arc<dyn Transport>, protocol: Protocol) -> io::Result<Self> {
        let (fs, sftp): (Arc<dyn RemoteFs>, _) = match protocol {
            Protocol::Sftp => {
                let fs = Arc::new(SftpFs::connect(transport.as_ref())?);
                (fs.clone(), Some(fs))
            }
            Protocol::Scp => (Arc::new(ShellFs::connect(transport.clone())?), None),
        };
        Ok(Self {
            transport,
            fs,
            sftp,
        })
    }

    pub fn fs(&self) -> Arc<dyn RemoteFs> {
        self.fs.clone()
    }

    /// Return an immutable-download identity; reject source changes or short reads.
    pub fn download(
        &self,
        path: &[u8],
        output: &mut dyn Write,
        progress: &mut dyn FnMut(u64, u64) -> bool,
    ) -> io::Result<FileInfo> {
        let info = self.fs.stat(path)?;
        if info.kind != crate::FileKind::File {
            return Err(io::Error::other("通常のファイルだけを編集できます"));
        }
        let got = match &self.sftp {
            Some(fs) => {
                let s = fs.sftp();
                let h = s.open(path, sftp::open::READ, &sftp::Attrs::default())?;
                let result = (|| {
                    let mut offset = 0;
                    loop {
                        if !progress(offset, info.len()) {
                            return Err(cancelled());
                        }
                        let data = s.send_read(&h, offset, sftp::CHUNK)?.wait()?;
                        if data.is_empty() {
                            break;
                        }
                        output.write_all(&data)?;
                        offset += data.len() as u64;
                        if offset > info.len() {
                            return Err(changed());
                        }
                    }
                    Ok(offset)
                })();
                let closed = s.close(&h);
                let got = result?;
                closed?;
                got
            }
            None => crate::scp::download(self.transport.as_ref(), path, 0, output, &mut |n| {
                progress(n, info.len())
            })?,
        };
        if got != info.len() || self.fs.stat(path)?.id != info.id {
            return Err(changed());
        }
        Ok(info)
    }

    /// Stage beside the destination, check for external changes twice, then replace.
    /// None means a conflict. Never delete the original to emulate atomic replacement.
    pub fn upload(
        &self,
        local: &std::path::Path,
        path: &[u8],
        expected: Option<FileId>,
        force: bool,
        progress: &mut dyn FnMut(u64, u64) -> bool,
    ) -> io::Result<Option<FileInfo>> {
        let current = self.fs.try_stat(path)?;
        if !force && current.as_ref().map(|i| i.id) != expected {
            return Ok(None);
        }
        if current
            .as_ref()
            .is_some_and(|i| i.kind != crate::FileKind::File || i.link)
        {
            return Err(io::Error::other(
                "通常のファイル以外やシンボリックリンクには上書きできません",
            ));
        }
        let token = tempfile::Builder::new().prefix("yy-save-").tempfile()?;
        let mut temp = yy_proto::parent_path(path);
        temp = crate::join_remote(
            &temp,
            token
                .path()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .as_bytes(),
        );
        let mut owns_temp = false;
        let result = (|| {
            let mut input = std::fs::File::open(local)?;
            let len = input.metadata()?.len();
            let mode = current.as_ref().map_or(0o600, |i| i.mode & 0o777);
            match &self.sftp {
                Some(fs) => {
                    let s = fs.sftp();
                    if current.is_some()
                        && !s
                            .extensions()
                            .iter()
                            .any(|(name, _)| name == "posix-rename@openssh.com")
                    {
                        return Err(io::Error::new(
                            io::ErrorKind::Unsupported,
                            "このSFTPサーバーは既存ファイルの安全な置換に未対応です（posix-rename拡張が必要）",
                        ));
                    }
                    let h = s.open(
                        &temp,
                        sftp::open::WRITE | sftp::open::CREATE | sftp::open::EXCLUSIVE,
                        &sftp::Attrs {
                            permissions: Some(mode),
                            ..Default::default()
                        },
                    )?;
                    owns_temp = true;
                    let sent = (|| {
                        let mut buf = vec![0; sftp::CHUNK as usize];
                        let mut offset = 0;
                        loop {
                            if !progress(offset, len) {
                                return Err(cancelled());
                            }
                            let n = input.read(&mut buf)?;
                            if n == 0 {
                                break;
                            }
                            s.send_write(&h, offset, &buf[..n])?.wait()?;
                            offset += n as u64;
                        }
                        s.fsync(&h)?;
                        if offset != len {
                            return Err(changed());
                        }
                        Ok(())
                    })();
                    let closed = s.close(&h);
                    sent?;
                    closed?;
                    s.setstat(
                        &temp,
                        &sftp::Attrs {
                            permissions: Some(mode),
                            ..Default::default()
                        },
                    )?;
                }
                None => {
                    // Reserve with noclobber before SCP opens the staging file.
                    let mut reserve = b"(set -C; : > ".to_vec();
                    reserve.extend(crate::shell_quote(&temp));
                    reserve.extend(b")");
                    crate::scp::shell(self.transport.as_ref(), &reserve, "一時ファイルの予約")?;
                    owns_temp = true;
                    crate::scp::upload(
                        self.transport.as_ref(),
                        &temp,
                        mode,
                        len,
                        &mut input,
                        &mut |n| progress(n, len),
                    )?;
                    let mut chmod = format!("chmod {mode:o} -- ").into_bytes();
                    chmod.extend(crate::shell_quote(&temp));
                    crate::scp::shell(self.transport.as_ref(), &chmod, "アクセス権の保持")?;
                }
            }
            if self.fs.stat(&temp)?.len() != len {
                return Err(changed());
            }
            if self.fs.try_stat(path)?.as_ref().map(|i| i.id) != current.as_ref().map(|i| i.id) {
                return Ok(None);
            }
            if !progress(len, len) {
                return Err(cancelled());
            }
            match &self.sftp {
                Some(fs) => fs.sftp().rename(&temp, path, current.is_some())?,
                None => {
                    let mut cmd = b"mv -T -f -- ".to_vec();
                    cmd.extend(crate::shell_quote(&temp));
                    cmd.push(b' ');
                    cmd.extend(crate::shell_quote(path));
                    crate::scp::shell(self.transport.as_ref(), &cmd, "編集ファイルの置換")?;
                }
            }
            self.fs.stat(path).map(Some)
        })();
        if owns_temp {
            let _ = self.fs.remove(&temp, false);
        }
        result
    }
}

fn changed() -> io::Error {
    io::Error::other("転送中にファイルが変更されたか、転送サイズが一致しません")
}
fn cancelled() -> io::Error {
    io::Error::new(io::ErrorKind::Interrupted, "中止しました")
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStrExt;
    struct SelectedOnly {
        local: crate::local::LocalTransport,
        protocol: Protocol,
    }
    impl Transport for SelectedOnly {
        fn exec(&self, command: &[u8]) -> io::Result<crate::Process> {
            assert_eq!(
                self.protocol,
                Protocol::Scp,
                "SFTP must not require shell/agent commands"
            );
            self.local.exec(command)
        }
        fn subsystem(&self, name: &str) -> io::Result<crate::Process> {
            assert_eq!(self.protocol, Protocol::Sftp, "SCP must never request SFTP");
            self.local.subsystem(name)
        }
        fn is_closed(&self) -> bool {
            false
        }
    }
    fn test(protocol: Protocol) {
        if protocol == Protocol::Sftp && crate::local::sftp_server().is_none() {
            assert!(
                std::env::var_os("YY_REQUIRE_SFTP").is_none(),
                "sftp-server is required"
            );
            eprintln!("sftp-server unavailable");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let local = tempfile::NamedTempFile::new().unwrap();
        let access = FileAccess::connect(
            Arc::new(SelectedOnly {
                local: crate::local::LocalTransport::new(),
                protocol,
            }),
            protocol,
        )
        .unwrap();
        let path = dir.path().join("日本語 ' & binary.dat");
        let remote = path.as_os_str().as_bytes();
        let bytes: Vec<_> = (0..(2 << 20)).map(|n| (n % 256) as u8).collect();
        std::fs::write(local.path(), &bytes).unwrap();
        let saved = access
            .upload(local.path(), remote, None, false, &mut |_, _| true)
            .unwrap()
            .unwrap();
        let mut read = Vec::new();
        assert_eq!(
            access
                .download(remote, &mut read, &mut |_, _| true)
                .unwrap()
                .len(),
            bytes.len() as u64
        );
        assert_eq!(read, bytes);
        // An unapproved overwrite and a canceled staged write preserve the destination.
        assert!(
            access
                .upload(local.path(), remote, None, false, &mut |_, _| true)
                .unwrap()
                .is_none()
        );
        std::fs::write(local.path(), b"changed").unwrap();
        assert_eq!(
            access
                .upload(local.path(), remote, Some(saved.id), false, &mut |_, _| {
                    false
                })
                .unwrap_err()
                .kind(),
            io::ErrorKind::Interrupted
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        let current = access
            .upload(local.path(), remote, Some(saved.id), false, &mut |_, _| {
                true
            })
            .unwrap()
            .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"changed");
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o640
        );
        // A change occurring during staging is detected, including force-save.
        let mut changed = false;
        assert!(
            access
                .upload(local.path(), remote, Some(current.id), true, &mut |_, _| {
                    if !changed {
                        std::fs::write(&path, b"external newer content").unwrap();
                        changed = true;
                    }
                    true
                })
                .unwrap()
                .is_none()
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"external newer content");
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            1,
            "staging files must be cleaned up"
        );
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(
            access
                .upload(
                    local.path(),
                    link.as_os_str().as_bytes(),
                    None,
                    true,
                    &mut |_, _| true
                )
                .is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"external newer content");
    }
    #[test]
    fn scp_editor_round_trip_conflicts_and_cancellation_without_sftp() {
        test(Protocol::Scp);
    }
    #[test]
    fn sftp_editor_round_trip_conflicts_and_cancellation_without_shell() {
        test(Protocol::Sftp);
    }
}
