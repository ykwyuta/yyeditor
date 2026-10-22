//! SCP-only hosts: browse and manage files through a POSIX shell and GNU tools.
//! No SFTP request and no agent deployment. File names remain byte strings.
use std::io;
use std::sync::Arc;

use crate::{DirEntry, FileId, FileInfo, FileKind, RemoteFs, Transport, run, shell_quote};

// A tag plus six NUL-delimited stat fields. Non-UTF8/newline names are preserved.
const INFO: &[u8] = br#"if [ -L "$p" ]; then printf 'link\0'; else printf 'plain\0'; fi; if [ ! -e "$p" ] && [ -L "$p" ]; then printf 'broken\0'; else stat -L --printf='%f\0%h\0%d\0%i\0%s\0%Y\0' -- "$p" || exit 74; fi"#;

pub struct ShellFs {
    transport: Arc<dyn Transport>,
    home: Vec<u8>,
}

impl ShellFs {
    /// The target (not the gateway OS) must provide sh, GNU find/stat and coreutils.
    pub fn connect(transport: Arc<dyn Transport>) -> io::Result<Self> {
        let out = checked(transport.as_ref(), b"printf '%s\\0' \"$HOME\"")?;
        let home = one_path(&out)?;
        Ok(Self { transport, home })
    }
    fn path(&self, path: &[u8]) -> io::Result<Vec<u8>> {
        if path.contains(&0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "パスにNULを含められません",
            ));
        }
        Ok(self.expand_home(path))
    }
    fn command(&self, command: &[u8]) -> io::Result<Vec<u8>> {
        checked(self.transport.as_ref(), command)
    }
}

fn checked(t: &dyn Transport, command: &[u8]) -> io::Result<Vec<u8>> {
    let out = run(t, command, b"")?;
    if out.success() {
        return Ok(out.stdout);
    }
    let kind = match out.status {
        Some(73) => io::ErrorKind::NotFound,
        Some(75) => io::ErrorKind::AlreadyExists,
        _ => io::ErrorKind::Other,
    };
    Err(io::Error::new(
        kind,
        format!(
            "SCPの一覧・ファイル操作には接続先のPOSIXシェルとGNU find/stat等が必要です: {}",
            out.message()
        ),
    ))
}

fn bad() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "SCP用シェルの応答が不正です（ログインメッセージやコマンドの互換性を確認してください）",
    )
}
fn one_path(out: &[u8]) -> io::Result<Vec<u8>> {
    let path = out.strip_suffix(&[0]).ok_or_else(bad)?;
    if !path.starts_with(b"/") || path.contains(&0) {
        return Err(bad());
    }
    Ok(path.to_vec())
}
fn field<'a>(fields: &mut impl Iterator<Item = &'a [u8]>) -> io::Result<&'a [u8]> {
    fields.next().ok_or_else(bad)
}
fn number(bytes: &[u8], radix: u32) -> io::Result<u64> {
    u64::from_str_radix(std::str::from_utf8(bytes).map_err(|_| bad())?, radix).map_err(|_| bad())
}
fn info<'a>(fields: &mut impl Iterator<Item = &'a [u8]>) -> io::Result<Option<FileInfo>> {
    let link = match field(fields)? {
        b"link" => true,
        b"plain" => false,
        _ => return Err(bad()),
    };
    let mode = field(fields)?;
    if link && mode == b"broken" {
        return Ok(None);
    }
    let mode = u32::try_from(number(mode, 16)?).map_err(|_| bad())?;
    let nlink = number(field(fields)?, 10)?;
    let dev = number(field(fields)?, 10)?;
    let ino = number(field(fields)?, 10)?;
    let len = number(field(fields)?, 10)?;
    let seconds = std::str::from_utf8(field(fields)?)
        .map_err(|_| bad())?
        .parse::<i64>()
        .map_err(|_| bad())?;
    Ok(Some(FileInfo {
        kind: match mode & 0o170000 {
            0o040000 => FileKind::Dir,
            0o100000 => FileKind::File,
            _ => FileKind::Other,
        },
        link,
        mode: mode & 0o7777,
        nlink,
        id: FileId {
            dev,
            ino,
            len,
            mtime_ns: seconds.saturating_mul(1_000_000_000),
        },
    }))
}

impl RemoteFs for ShellFs {
    fn kind(&self) -> &'static str {
        "SCP・シェル"
    }
    fn home(&self) -> &[u8] {
        &self.home
    }
    fn is_closed(&self) -> bool {
        self.transport.is_closed()
    }
    fn real_path(&self, path: &[u8]) -> io::Result<Vec<u8>> {
        let mut cmd = b"realpath -e -z -- ".to_vec();
        cmd.extend(shell_quote(&self.path(path)?));
        one_path(&self.command(&cmd)?)
    }
    fn stat(&self, path: &[u8]) -> io::Result<FileInfo> {
        let mut cmd = b"p=".to_vec();
        cmd.extend(shell_quote(&self.path(path)?));
        cmd.extend_from_slice(b"; if [ ! -e \"$p\" ] && [ ! -L \"$p\" ]; then exit 73; fi; ");
        cmd.extend_from_slice(INFO);
        let out = self.command(&cmd)?;
        let mut fields = out.strip_suffix(&[0]).ok_or_else(bad)?.split(|&b| b == 0);
        let result = info(&mut fields)?
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "リンク先がありません"))?;
        if fields.next().is_some() {
            return Err(bad());
        }
        Ok(result)
    }
    fn read_dir(&self, path: &[u8]) -> io::Result<Vec<DirEntry>> {
        let mut script = b"for p do printf '%s\\0' \"${p##*/}\"; ".to_vec();
        script.extend_from_slice(INFO);
        script.extend_from_slice(b"; done");
        let mut cmd = b"LC_ALL=C find -H ".to_vec();
        cmd.extend(shell_quote(&self.path(path)?));
        cmd.extend_from_slice(b" -mindepth 1 -maxdepth 1 -exec sh -c ");
        cmd.extend(shell_quote(&script));
        cmd.extend_from_slice(b" sh {} +");
        let out = self.command(&cmd)?;
        if out.is_empty() {
            return Ok(vec![]);
        }
        let mut fields = out.strip_suffix(&[0]).ok_or_else(bad)?.split(|&b| b == 0);
        let mut entries = Vec::new();
        while let Some(name) = fields.next() {
            if name.is_empty() || name == b"." || name == b".." || name.contains(&b'/') {
                return Err(bad());
            }
            entries.push(DirEntry {
                name: name.to_vec(),
                info: info(&mut fields)?,
            });
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(entries)
    }
    fn make_dir(&self, path: &[u8]) -> io::Result<()> {
        let mut cmd = b"mkdir -- ".to_vec();
        cmd.extend(shell_quote(&self.path(path)?));
        self.command(&cmd).map(|_| ())
    }
    fn rename(&self, from: &[u8], to: &[u8]) -> io::Result<()> {
        let from = shell_quote(&self.path(from)?);
        let to = shell_quote(&self.path(to)?);
        let mut cmd = b"if [ -e ".to_vec();
        cmd.extend(&to);
        cmd.extend_from_slice(b" ] || [ -L ");
        cmd.extend(&to);
        cmd.extend_from_slice(b" ]; then exit 75; fi; mv -nT -- ");
        cmd.extend(&from);
        cmd.push(b' ');
        cmd.extend(&to);
        cmd.extend_from_slice(b" && [ ! -e ");
        cmd.extend(&from);
        cmd.extend_from_slice(b" ] && [ ! -L ");
        cmd.extend(&from);
        cmd.extend_from_slice(b" ]");
        self.command(&cmd).map(|_| ())
    }
    fn remove(&self, path: &[u8], recursive: bool) -> io::Result<()> {
        let path = self.path(path)?;
        let mut cmd = if recursive {
            b"rm -r --preserve-root=all -- ".to_vec()
        } else {
            let mut c = b"if [ -d ".to_vec();
            c.extend(shell_quote(&path));
            c.extend_from_slice(b" ] && [ ! -L ");
            c.extend(shell_quote(&path));
            c.extend_from_slice(b" ]; then rmdir -- ");
            c.extend(shell_quote(&path));
            c.extend_from_slice(b"; else rm -- ");
            c
        };
        cmd.extend(shell_quote(&path));
        if !recursive {
            cmd.extend_from_slice(b"; fi");
        }
        self.command(&cmd).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Exit, Process};
    use std::io::Cursor;
    struct Fake {
        commands: std::sync::Mutex<Vec<Vec<u8>>>,
    }
    impl Transport for Fake {
        fn is_closed(&self) -> bool {
            false
        }
        fn exec(&self, cmd: &[u8]) -> io::Result<Process> {
            self.commands.lock().unwrap().push(cmd.to_vec());
            let out = if cmd.starts_with(b"printf") {
                b"/home/test\x00".to_vec()
            } else if cmd.starts_with(b"realpath") {
                b"/home/test/a b\x00".to_vec()
            } else if cmd.starts_with(b"LC_ALL") {
                b"a\nb\xff\x00plain\x0081a4\x001\x001\x002\x003\x001700000000\x00broken link\x00link\x00broken\x00"
                    .to_vec()
            } else if cmd.starts_with(b"p=") {
                b"plain\x0081a4\x001\x001\x002\x003\x001700000000\x00".to_vec()
            } else {
                vec![]
            };
            Ok(Process::new(
                Box::new(io::sink()),
                Box::new(Cursor::new(out)),
                Box::new(|| {
                    Ok(Exit {
                        status: Some(0),
                        stderr: vec![],
                    })
                }),
            ))
        }
        fn subsystem(&self, _: &str) -> io::Result<Process> {
            panic!("SCP must not use SFTP")
        }
    }
    #[test]
    fn scp_browsing_needs_no_subsystem_and_preserves_binary_names() {
        let t = Arc::new(Fake {
            commands: Default::default(),
        });
        let fs = ShellFs::connect(t.clone()).unwrap();
        assert_eq!(fs.real_path(b"a b").unwrap(), b"/home/test/a b");
        assert_eq!(fs.stat(b"x").unwrap().len(), 3);
        let entries = fs.read_dir(b".").unwrap();
        assert_eq!(entries[0].name, b"a\nb\xff");
        assert!(entries[1].info.is_none());
        fs.make_dir(b"a'; touch bad").unwrap();
        fs.rename(b"from", b"to").unwrap();
        fs.remove(b"dir", true).unwrap();
        fs.remove(b"file", false).unwrap();
        assert!(fs.stat(b"bad\x00name").is_err());
        assert!(
            t.commands
                .lock()
                .unwrap()
                .iter()
                .any(|c| c.ends_with(b"'/home/test/a'\\''; touch bad'"))
        );
    }

    #[test]
    fn shell_file_operations_work_with_real_gnu_tools() {
        struct RealShell {
            executable: std::path::PathBuf,
            dir: std::path::PathBuf,
        }
        impl Transport for RealShell {
            fn is_closed(&self) -> bool {
                false
            }
            fn exec(&self, command: &[u8]) -> io::Result<Process> {
                let script = format!(
                    "PATH=/usr/bin:/bin; export PATH; HOME=$(pwd); export HOME; {}",
                    String::from_utf8_lossy(command)
                );
                let output = std::process::Command::new(&self.executable)
                    .current_dir(&self.dir)
                    .args(["-c", &script])
                    .output()?;
                let exit = Exit {
                    status: output.status.code().map(|n| n as u32),
                    stderr: output.stderr,
                };
                Ok(Process::new(
                    Box::new(io::sink()),
                    Box::new(Cursor::new(output.stdout)),
                    Box::new(move || Ok(exit)),
                ))
            }
            fn subsystem(&self, _: &str) -> io::Result<Process> {
                panic!("SCP must not use SFTP")
            }
        }
        let executable = if cfg!(windows) {
            std::path::PathBuf::from("C:/Program Files/Git/usr/bin/sh.exe")
        } else {
            std::path::PathBuf::from("/bin/sh")
        };
        if !executable.is_file() {
            eprintln!("GNUシェル環境がないため実コマンド試験は省略");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let fs = ShellFs::connect(Arc::new(RealShell {
            executable,
            dir: dir.path().to_owned(),
        }))
        .unwrap();
        let name = "a b'日本語.txt";
        std::fs::write(dir.path().join(name), b"binary\x00\xff").unwrap();
        std::fs::write(dir.path().join(".hidden"), b"hidden").unwrap();
        let items = fs.read_dir(b".").unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(fs.stat(name.as_bytes()).unwrap().len(), 8);
        assert!(
            fs.real_path(name.as_bytes())
                .unwrap()
                .ends_with(name.as_bytes())
        );
        assert!(
            fs.stat(b"absent")
                .is_err_and(|e| e.kind() == io::ErrorKind::NotFound)
        );
        fs.make_dir(b"folder").unwrap();
        fs.rename(name.as_bytes(), b"folder/renamed").unwrap();
        assert!(dir.path().join("folder/renamed").is_file());
        assert!(
            fs.rename(b".hidden", b"folder/renamed")
                .is_err_and(|e| e.kind() == io::ErrorKind::AlreadyExists)
        );
        assert_eq!(
            std::fs::read(dir.path().join("folder/renamed")).unwrap(),
            b"binary\x00\xff"
        );
        fs.remove(b"folder", true).unwrap();
        fs.remove(b".hidden", false).unwrap();
        assert!(fs.read_dir(b".").unwrap().is_empty());
    }
}
