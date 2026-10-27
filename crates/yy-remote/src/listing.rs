//! Optional host-specific directory listing, independent of transfer protocol.
use std::collections::HashSet;
use std::io;
use std::sync::Arc;

use crate::{DirEntry, FileInfo, RemoteFs, Transport, run, shell_quote};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListCommand {
    command: String,
    nul: bool,
    long: bool,
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
        let (nul, long) = match format.unwrap_or("lines") {
            "lines" => (false, false),
            "nul" => (true, false),
            "ls" => (false, true),
            _ => return Err(invalid("list_formatはlines、nul、lsを指定してください")),
        };
        Ok(Some(Self {
            command: command.to_owned(),
            nul,
            long,
        }))
    }

    fn names(&self, output: &[u8]) -> io::Result<Vec<Vec<u8>>> {
        if self.long {
            let mut names = Vec::new();
            for line in output.split(|&b| b == b'\n') {
                let line = line.strip_suffix(b"\r").unwrap_or(line);
                if line.is_empty() {
                    continue;
                }
                if let Some(name) = ls_name(line)? {
                    names.extend(name);
                    names.push(b'\n');
                }
            }
            return Self {
                command: String::new(),
                nul: false,
                long: false,
            }
            .names(&names);
        }
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

fn ls_name(mut line: &[u8]) -> io::Result<Option<&[u8]>> {
    fn bad() -> io::Error {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "ls長形式の一覧を解析できません（ls -la、LC_ALL=C、引用形式、バナーを確認してください）",
        )
    }
    fn token<'a>(line: &mut &'a [u8]) -> io::Result<&'a [u8]> {
        *line = line.trim_ascii_start();
        let end = line
            .iter()
            .position(u8::is_ascii_whitespace)
            .unwrap_or(line.len());
        if end == 0 {
            return Err(bad());
        }
        let value = &line[..end];
        *line = &line[end..];
        Ok(value)
    }
    if let Some(total) = line.strip_prefix(b"total ") {
        if total.trim_ascii().iter().all(u8::is_ascii_digit) {
            return Ok(None);
        }
        return Err(bad());
    }
    let mode = token(&mut line)?;
    if !(10..=11).contains(&mode.len())
        || !b"-dlbcps".contains(&mode[0])
        || mode[1..10].iter().any(|b| !b"rwxstST-".contains(b))
    {
        return Err(bad());
    }
    for _ in 0..3 {
        token(&mut line)?;
    } // link count, owner, group
    let size = token(&mut line)?;
    if matches!(mode[0], b'b' | b'c') && size.ends_with(b",") {
        token(&mut line)?;
    }
    let date = token(&mut line)?;
    let iso = date.len() == 10 && date[4] == b'-' && date[7] == b'-';
    token(&mut line)?;
    if !iso {
        token(&mut line)?;
    } else {
        let probe = line.trim_ascii_start();
        let zone = probe
            .split(u8::is_ascii_whitespace)
            .next()
            .unwrap_or_default();
        if zone.len() == 5
            && matches!(zone[0], b'+' | b'-')
            && zone[1..].iter().all(u8::is_ascii_digit)
        {
            token(&mut line)?;
        }
    }
    let mut name = line.trim_ascii_start();
    if mode[0] == b'l' {
        let delimiter = b" -> ";
        let mut positions = name
            .windows(delimiter.len())
            .enumerate()
            .filter_map(|(i, w)| (w == delimiter).then_some(i));
        let end = positions.next().ok_or_else(bad)?;
        if positions.next().is_some() {
            return Err(bad());
        }
        name = &name[..end];
    }
    if name.is_empty() || name.starts_with(b"'") || name.starts_with(b"\"") || name.contains(&b'\\')
    {
        return Err(bad());
    }
    Ok(Some(name))
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
        let span = self.transport.diagnostics().span("directory.list");
        span.metadata(&[
            ("custom_command", crate::diagnostics::Value::Flag(true)),
            (
                "format",
                crate::diagnostics::Value::Code(if self.listing.long {
                    "ls"
                } else if self.listing.nul {
                    "nul"
                } else {
                    "lines"
                }),
            ),
        ]);
        span.run(|| self.read_dir_inner(path))
    }
}

impl CommandListFs {
    fn read_dir_inner(&self, path: &[u8]) -> io::Result<Vec<DirEntry>> {
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
    fn ls_long_names_support_spaces_links_iso_dates_and_reject_banners_and_paths() {
        let listing = ListCommand::from_settings(Some("ls -la"), Some("ls"))
            .unwrap()
            .unwrap();
        let data = "total 12\n-rw-r--r-- 1 user group 12 Oct 25 09:40 a b.txt\nlrwxrwxrwx 1 user group 3 Oct 25 2026 link -> a b.txt\ndrwxr-xr-x 2 user group 4096 2026-10-25 09:40:12.000 +0900 日本語\n-rw-r--r-- 1 u g 0 2026-10-25 09:40 long-iso\n";
        assert_eq!(
            listing.names(data.as_bytes()).unwrap(),
            [
                b"a b.txt".to_vec(),
                b"link".to_vec(),
                "日本語".as_bytes().to_vec(),
                b"long-iso".to_vec()
            ]
        );
        for bad in [
            b"Welcome to gateway\n".as_slice(),
            b"-rw-r--r-- 1 u g 1 Oct 25 12:30 ../escape\n",
            b"-rw-r--r-- 1 u g 1 Oct 25 12:30 'quoted name'\n",
        ] {
            assert!(listing.names(bad).is_err());
        }
    }
    #[cfg(unix)]
    #[test]
    fn real_ls_long_listing_retains_names_and_metadata() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a b.txt"), b"abc").unwrap();
        std::fs::create_dir(dir.path().join("日本語")).unwrap();
        std::os::unix::fs::symlink("a b.txt", dir.path().join("link")).unwrap();
        let transport = Arc::new(crate::local::LocalTransport::new());
        let inner = Arc::new(crate::shellfs::ShellFs::connect(transport.clone()).unwrap());
        let listing = ListCommand::from_settings(Some("LC_ALL=C ls -la"), Some("ls"))
            .unwrap()
            .unwrap();
        let fs = CommandListFs::new(inner, transport, listing);
        use std::os::unix::ffi::OsStrExt;
        let entries = fs.read_dir(dir.path().as_os_str().as_bytes()).unwrap();
        assert_eq!(entries.len(), 3);
        assert!(
            entries
                .iter()
                .any(|e| e.name == b"a b.txt" && e.info.as_ref().unwrap().id.len == 3)
        );
        assert!(
            entries
                .iter()
                .any(|e| e.name == b"link" && e.info.as_ref().unwrap().link)
        );
        assert!(entries.iter().any(|e| e.name == "日本語".as_bytes()));
    }
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
