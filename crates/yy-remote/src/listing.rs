//! Optional host-specific directory listing, independent of transfer protocol.
use std::collections::HashSet;
use std::io;
use std::sync::Arc;

use crate::{DirEntry, FileInfo, RemoteFs, Transport, run, shell_quote};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListCommand {
    command: String,
    nul: bool,
}

impl ListCommand {
    pub fn from_settings(command: Option<&str>, format: Option<&str>) -> io::Result<Option<Self>> {
        let invalid = |message| io::Error::new(io::ErrorKind::InvalidInput, message);
        let Some(command) = command else {
            return if format.is_some() {
                Err(invalid("list_formatにはlist_commandの指定が必要です"))
            } else {
                Ok(None)
            };
        };
        if command.trim().is_empty() || command.contains('\0') {
            return Err(invalid("list_commandに空文字列やNULは指定できません"));
        }
        let nul = match format.unwrap_or("lines") {
            "lines" => false,
            "nul" => true,
            _ => return Err(invalid("list_formatはlinesまたはnulを指定してください")),
        };
        Ok(Some(Self {
            command: command.to_owned(),
            nul,
        }))
    }

    fn names(&self, output: &[u8]) -> io::Result<Vec<Vec<u8>>> {
        if output.is_empty() {
            return Ok(vec![]);
        }
        let invalid = || {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "list_commandの出力は直下のファイル名だけにしてください（list_formatの区切り、空行、重複、パスを確認してください）",
            )
        };
        let separator = if self.nul { 0 } else { b'\n' };
        let body = if self.nul {
            output.strip_suffix(&[0]).ok_or_else(invalid)?
        } else {
            output.strip_suffix(b"\n").unwrap_or(output)
        };
        let mut names = Vec::new();
        let mut seen = HashSet::new();
        for name in body.split(|&b| b == separator) {
            let name = if self.nul {
                name
            } else {
                name.strip_suffix(b"\r").unwrap_or(name)
            };
            if matches!(name, b"." | b"..") {
                continue;
            }
            if name.is_empty()
                || name.contains(&0)
                || name.contains(&b'/')
                || !seen.insert(name.to_vec())
            {
                return Err(invalid());
            }
            names.push(name.to_vec());
        }
        Ok(names)
    }
}

/// Override read_dir only; metadata and all other operations retain their backend.
pub struct CommandListFs {
    inner: Arc<dyn RemoteFs>,
    transport: Arc<dyn Transport>,
    listing: ListCommand,
}

impl CommandListFs {
    pub fn new(
        inner: Arc<dyn RemoteFs>,
        transport: Arc<dyn Transport>,
        listing: ListCommand,
    ) -> Self {
        Self {
            inner,
            transport,
            listing,
        }
    }
}

impl RemoteFs for CommandListFs {
    fn kind(&self) -> &'static str {
        self.inner.kind()
    }
    fn home(&self) -> &[u8] {
        self.inner.home()
    }
    fn is_closed(&self) -> bool {
        self.inner.is_closed() || self.transport.is_closed()
    }
    fn real_path(&self, path: &[u8]) -> io::Result<Vec<u8>> {
        self.inner.real_path(path)
    }
    fn stat(&self, path: &[u8]) -> io::Result<FileInfo> {
        self.inner.stat(path)
    }
    fn make_dir(&self, path: &[u8]) -> io::Result<()> {
        self.inner.make_dir(path)
    }
    fn rename(&self, from: &[u8], to: &[u8]) -> io::Result<()> {
        self.inner.rename(from, to)
    }
    fn remove(&self, path: &[u8], recursive: bool) -> io::Result<()> {
        self.inner.remove(path, recursive)
    }
    fn hash(&self, path: &[u8], len: u64) -> io::Result<Option<Vec<u8>>> {
        self.inner.hash(path, len)
    }
    fn read_dir(&self, path: &[u8]) -> io::Result<Vec<DirEntry>> {
        let path = self.inner.expand_home(path);
        if path.contains(&0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "ディレクトリにNULを含められません",
            ));
        }
        let quoted = shell_quote(&path);
        // The configured command is trusted shell code. Quote only the directory,
        // and stop before the command if cd fails. No placeholder substitution.
        let mut command = b"cd ".to_vec();
        command.extend(&quoted);
        command.extend_from_slice(b" || exit $?; YY_LIST_DIRECTORY=");
        command.extend(&quoted);
        command.extend_from_slice(b"; export YY_LIST_DIRECTORY;\n");
        command.extend_from_slice(self.listing.command.as_bytes());
        let output = run(self.transport.as_ref(), &command, b"")?;
        if !output.success() {
            return Err(io::Error::other(format!(
                "list_commandが失敗しました（終了コード {:?}）: {}",
                output.status,
                output.message()
            )));
        }
        // Validate the entire response before issuing metadata requests.
        let names = self.listing.names(&output.stdout)?;
        let mut entries = Vec::with_capacity(names.len());
        for name in names {
            let info = match self.inner.stat(&crate::join_remote(&path, &name)) {
                Ok(info) => Some(info),
                Err(e) if e.kind() == io::ErrorKind::NotFound => None,
                Err(e) => return Err(e),
            };
            entries.push(DirEntry { name, info });
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn formats_preserve_names_and_reject_paths_duplicates_and_malformed_output() {
        let lines = ListCommand::from_settings(Some("ls -1A"), None)
            .unwrap()
            .unwrap();
        assert_eq!(
            lines
                .names(".\r\n..\r\n a b \r\n日本語\n".as_bytes())
                .unwrap(),
            vec![b" a b ".to_vec(), "日本語".as_bytes().to_vec()]
        );
        let nul = ListCommand::from_settings(Some("script"), Some("nul"))
            .unwrap()
            .unwrap();
        assert_eq!(
            nul.names(b"a\nb\x00\xff\x00").unwrap(),
            vec![b"a\nb".to_vec(), vec![255]]
        );
        assert!(lines.names(b"").unwrap().is_empty());
        for output in [
            b"../escape\n".as_slice(),
            b"/absolute\n",
            b"a\na\n",
            b"a\n\n",
            b"a\x00b\n",
        ] {
            assert!(lines.names(output).is_err());
        }
        assert!(nul.names(b"missing terminator").is_err());
        assert!(ListCommand::from_settings(None, Some("lines")).is_err());
        assert!(ListCommand::from_settings(Some(""), None).is_err());
        assert!(ListCommand::from_settings(Some("ls"), Some("unknown")).is_err());
        assert!(ListCommand::from_settings(Some("ls\0"), None).is_err());
        assert!(ListCommand::from_settings(None, None).unwrap().is_none());
    }

    #[test]
    fn command_errors_do_not_fall_back_and_invalid_names_never_reach_metadata() {
        use crate::{Exit, FileId, FileKind, Process};
        struct Backend(std::sync::Mutex<Vec<Vec<u8>>>);
        impl RemoteFs for Backend {
            fn kind(&self) -> &'static str {
                "SFTP"
            }
            fn home(&self) -> &[u8] {
                b"/home/quoted 'dir"
            }
            fn is_closed(&self) -> bool {
                false
            }
            fn real_path(&self, p: &[u8]) -> io::Result<Vec<u8>> {
                Ok(self.expand_home(p))
            }
            fn stat(&self, p: &[u8]) -> io::Result<FileInfo> {
                self.0.lock().unwrap().push(p.to_vec());
                Ok(FileInfo {
                    kind: FileKind::File,
                    link: false,
                    mode: 0o644,
                    nlink: 1,
                    id: FileId {
                        dev: 0,
                        ino: 0,
                        len: 8,
                        mtime_ns: 0,
                    },
                })
            }
            fn read_dir(&self, _: &[u8]) -> io::Result<Vec<DirEntry>> {
                panic!("must not fall back")
            }
            fn make_dir(&self, _: &[u8]) -> io::Result<()> {
                unreachable!()
            }
            fn rename(&self, _: &[u8], _: &[u8]) -> io::Result<()> {
                unreachable!()
            }
            fn remove(&self, _: &[u8], _: bool) -> io::Result<()> {
                unreachable!()
            }
        }
        struct Wire {
            data: Vec<u8>,
            status: u32,
            commands: std::sync::Mutex<Vec<Vec<u8>>>,
        }
        impl Transport for Wire {
            fn is_closed(&self) -> bool {
                false
            }
            fn exec(&self, c: &[u8]) -> io::Result<Process> {
                self.commands.lock().unwrap().push(c.to_vec());
                let status = self.status;
                Ok(Process::new(
                    Box::new(io::sink()),
                    Box::new(io::Cursor::new(self.data.clone())),
                    Box::new(move || {
                        Ok(Exit {
                            status: Some(status),
                            stderr: vec![],
                        })
                    }),
                ))
            }
        }
        let listing = ListCommand::from_settings(Some("custom '%s'"), None)
            .unwrap()
            .unwrap();
        for (data, status, success) in [
            (b"file name\n".as_slice(), 0, true),
            (b"file\n../escape\n", 0, false),
            (b"file\n", 1, false),
        ] {
            let backend = Arc::new(Backend(Default::default()));
            let wire = Arc::new(Wire {
                data: data.to_vec(),
                status,
                commands: Default::default(),
            });
            let fs = CommandListFs::new(backend.clone(), wire.clone(), listing.clone());
            let result = fs.read_dir(b".");
            assert_eq!(result.is_ok(), success);
            assert_eq!(backend.0.lock().unwrap().len(), usize::from(success));
            let command = wire.commands.lock().unwrap()[0].clone();
            assert!(command.starts_with(b"cd '/home/quoted '\\''dir/.' || exit $?;"));
            assert!(command.ends_with(b"custom '%s'"));
            assert!(fs.read_dir(b"bad\0path").is_err());
            assert_eq!(wire.commands.lock().unwrap().len(), 1);
        }
    }
}
