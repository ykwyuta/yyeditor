//! WSL discovery and narrowly scoped Linux ownership/mode changes.
use std::os::windows::process::CommandExt;
use std::{
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use yy_files::wsl::{Location, Permissions, STAT_FORMAT};

#[path = "wsl_dialog.rs"]
mod dialog;
pub(super) use dialog::open;

pub(super) struct Secret(Vec<u8>);
impl Secret {
    fn new(value: String) -> Self {
        Self(value.into_bytes())
    }
    fn text(&self) -> &str {
        std::str::from_utf8(&self.0).unwrap_or("")
    }
}
impl Drop for Secret {
    fn drop(&mut self) {
        for byte in &mut self.0 {
            unsafe {
                std::ptr::write_volatile(byte, 0);
            }
        }
        std::sync::atomic::compiler_fence(Ordering::SeqCst);
    }
}
fn program() -> PathBuf {
    std::env::var_os("WINDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
        .join("System32/wsl.exe")
}
struct Child(std::process::Child);
impl Drop for Child {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Output pipes stay in memory. Secrets go only to stdin, never argv or files.
fn capture(
    command: &mut Command,
    secret: Option<&Secret>,
    cancel: &AtomicBool,
) -> io::Result<Vec<u8>> {
    if cancel.load(Ordering::Relaxed) {
        return Err(yy_files::cancelled());
    }
    if secret
        .is_some_and(|s| s.0.len() > 2048 || s.0.iter().any(|b| matches!(*b, b'\r' | b'\n' | 0)))
    {
        return Err(io::Error::other(
            "パスワードは改行なし・2048バイト以内にしてください",
        ));
    }
    let mut child = Child(
        command
            .stdin(if secret.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(0x08000000)
            .spawn()?,
    );
    let overflow = Arc::new(AtomicBool::new(false));
    let read = |pipe: Box<dyn Read + Send>, overflow: Arc<AtomicBool>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let result = pipe.take(65_537).read_to_end(&mut bytes);
            if bytes.len() > 65_536 {
                overflow.store(true, Ordering::Relaxed);
            }
            result.map(|_| bytes)
        })
    };
    let stdout = read(Box::new(child.0.stdout.take().unwrap()), overflow.clone());
    let stderr = read(Box::new(child.0.stderr.take().unwrap()), overflow.clone());
    let input = if let Some(secret) = secret {
        let mut pipe = child.0.stdin.take().unwrap();
        pipe.write_all(&secret.0)
            .and_then(|_| pipe.write_all(b"\n"))
    } else {
        Ok(())
    };
    let start = Instant::now();
    let result = (|| {
        input?;
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(yy_files::cancelled());
            }
            if overflow.load(Ordering::Relaxed) {
                return Err(io::Error::other("WSLの出力が64 KiBを超えました"));
            }
            if start.elapsed() > Duration::from_secs(30) {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "WSLの応答が30秒以内に返りません",
                ));
            }
            if let Some(status) = child.0.try_wait()? {
                return Ok(status);
            }
            std::thread::park_timeout(Duration::from_millis(10));
        }
    })();
    let _ = child.0.kill();
    let _ = child.0.wait();
    let output = stdout.join();
    let diagnostic = stderr.join();
    let output = output.map_err(|_| io::Error::other("WSL出力の読み取り失敗"))?;
    let _diagnostic = diagnostic.map_err(|_| io::Error::other("WSL出力の読み取り失敗"))?;
    let status = result?;
    if !status.success() {
        // Never include a privileged process's raw output in logs or dialogs.
        return Err(io::Error::other(format!(
            "WSLの操作に失敗しました（終了コード {:?}）。対象の存在・sudoの認証と許可・coreutilsを確認してください。途中まで変更されている可能性があるため、再読み込みで実際の権限を確認してください。",
            status.code()
        )));
    }
    let output = output?;
    if output.len() > 65_536 {
        return Err(io::Error::other("WSLの出力が上限を超えました"));
    }
    Ok(output)
}

pub(super) fn distributions() -> io::Result<Vec<String>> {
    let bytes = capture(
        Command::new(program()).args(["--list", "--quiet"]),
        None,
        &AtomicBool::new(false),
    )?;
    yy_files::wsl::decode_distributions(&bytes)
}
pub(super) fn location(path: &Path) -> Option<Location> {
    Location::parse(&path.to_string_lossy())
}

pub(super) fn exclude_size_mount(root: &Path, path: &Path) -> bool {
    let (Some(root), Some(path)) = (location(root), location(path)) else {
        return false;
    };
    root.linux != path.linux && matches!(path.linux.as_str(), "/proc" | "/sys" | "/dev" | "/run")
}
fn command(loc: &Location, user: Option<&str>) -> Command {
    let mut cmd = Command::new(program());
    cmd.args(["--distribution", &loc.distro]);
    if let Some(user) = user {
        cmd.args(["--user", user]);
    }
    cmd.args([
        "--exec",
        "/usr/bin/timeout",
        "--kill-after=2s",
        "20s",
        "/usr/bin/env",
        "LC_ALL=C",
    ]);
    cmd
}
const READ_SCRIPT: &str = "set -eu\nu=$(/usr/bin/id -un)\nprintf '%s\\0' \"$u\"\nexec /usr/bin/stat --printf \"$2\" -- \"$1\"";
pub(super) struct Info {
    pub user: String,
    pub permissions: Permissions,
}
pub(super) fn read(loc: &Location, cancel: &AtomicBool) -> io::Result<Info> {
    let bytes = capture(
        command(loc, None).args([
            "/bin/sh",
            "-c",
            READ_SCRIPT,
            "yyfilemanager",
            &loc.linux,
            STAT_FORMAT,
        ]),
        None,
        cancel,
    )?;
    let separator = bytes
        .iter()
        .position(|b| *b == 0)
        .ok_or_else(|| io::Error::other("WSLユーザー情報が不正です"))?;
    let user = String::from_utf8(bytes[..separator].to_vec()).map_err(io::Error::other)?;
    yy_files::wsl::identity(&user)?;
    Ok(Info {
        user,
        permissions: Permissions::parse(&bytes[separator + 1..])?,
    })
}

// Open and verify the inode after sudo, then operate on its held descriptor.
// A path swapped for a symlink after the GUI read cannot redirect the change.
const APPLY_SCRIPT: &str = r#"set -eu
[ ! -L "$1" ] || exit 65
exec 3< "$1"
[ "$(/usr/bin/stat -L -c '%d:%i' -- /proc/self/fd/3)" = "$5" ] || exit 66
if [ "$7" = 1 ]; then /usr/bin/chown --dereference -- "$2:$3" /proc/self/fd/3; fi
if [ "$8" = 1 ]; then /usr/bin/chmod -- "$4" /proc/self/fd/3; fi
exec /usr/bin/stat -L --printf "$6" -- /proc/self/fd/3
"#;
pub(super) struct Change {
    pub owner: String,
    pub group: String,
    pub mode: u32,
    pub expected: Permissions,
}
impl Change {
    pub(super) fn new(
        owner: &str,
        group: &str,
        mode: &str,
        expected: &Permissions,
    ) -> io::Result<Self> {
        if !matches!(
            expected.kind.as_str(),
            "regular file" | "regular empty file" | "directory"
        ) || expected.symlink()
        {
            return Err(io::Error::other(
                "変更対象はリンクでない通常ファイル・フォルダにしてください",
            ));
        }
        Ok(Self {
            owner: yy_files::wsl::identity(owner)?,
            group: yy_files::wsl::identity(group)?,
            mode: yy_files::wsl::mode(mode)?,
            expected: expected.clone(),
        })
    }
}
fn apply_as(
    loc: &Location,
    change: &Change,
    secret: Option<&Secret>,
    user: Option<&str>,
    cancel: &AtomicBool,
) -> io::Result<Permissions> {
    let mut cmd = command(loc, user);
    if secret.is_some() {
        cmd.args(["/usr/bin/sudo", "-S", "-k", "-p", "", "--"]);
    }
    let same_identity = |value: &str, id: u32, name: &str| {
        if value.bytes().all(|b| b.is_ascii_digit()) {
            value.parse::<u32>().ok() == Some(id)
        } else {
            value == name
        }
    };
    let owner_changed = !same_identity(&change.owner, change.expected.uid, &change.expected.owner);
    let group_changed = !same_identity(&change.group, change.expected.gid, &change.expected.group);
    let chown = owner_changed || group_changed;
    let account = |value: &str| {
        if value.bytes().all(|b| b.is_ascii_digit()) {
            format!("+{value}")
        } else {
            value.to_owned()
        }
    };
    cmd.args([
        "/bin/sh",
        "-c",
        APPLY_SCRIPT,
        "yyfilemanager",
        &loc.linux,
        &account(&change.owner),
        &account(&change.group),
        &format!("{:04o}", change.mode),
        &change.expected.fingerprint,
        STAT_FORMAT,
        if chown { "1" } else { "0" },
        if chown || change.mode != change.expected.mode {
            "1"
        } else {
            "0"
        },
    ]);
    let bytes = capture(&mut cmd, secret, cancel)?;
    let result = Permissions::parse(&bytes)?;
    let owner_matches = if change.owner.bytes().all(|b| b.is_ascii_digit()) {
        change.owner.parse::<u32>().ok() == Some(result.uid)
    } else {
        change.owner == result.owner
    };
    let group_matches = if change.group.bytes().all(|b| b.is_ascii_digit()) {
        change.group.parse::<u32>().ok() == Some(result.gid)
    } else {
        change.group == result.group
    };
    if result.mode != change.mode
        || result.fingerprint != change.expected.fingerprint
        || !owner_matches
        || !group_matches
    {
        return Err(io::Error::other(
            "要求した権限が反映されていません。/mnt配下ではWSLのmetadataマウント設定とWindows側の権限も確認してください。再読み込みで実際の値を確認してください",
        ));
    }
    Ok(result)
}
pub(super) fn apply(
    loc: &Location,
    user: &str,
    change: &Change,
    secret: Option<&Secret>,
    cancel: &AtomicBool,
) -> io::Result<Permissions> {
    yy_files::wsl::identity(user)?;
    apply_as(loc, change, secret, Some(user), cancel)
}

#[cfg(test)]
#[path = "wsl_tests.rs"]
mod tests;
