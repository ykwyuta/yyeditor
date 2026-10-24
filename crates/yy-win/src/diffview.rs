//! Compare immutable editor/Git snapshots in the bundled yymerge application.

use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
use windows::core::HSTRING;
use yy_buffer::Snapshot;

struct Inputs {
    directory: tempfile::TempDir,
    left: PathBuf,
    right: PathBuf,
}

fn write_snapshot(path: &Path, snapshot: &Snapshot) -> io::Result<()> {
    let mut file = BufWriter::new(File::create(path)?);
    // yymerge consumes one leading signature. Adding our own preserves a real
    // U+FEFF at the beginning of the editor document, including unsaved inserts.
    file.write_all(b"\xEF\xBB\xBF")?;
    for slice in snapshot.slices(0..snapshot.len()) {
        file.write_all(slice.bytes)?;
    }
    file.flush()
}

impl Inputs {
    fn new(left: &Snapshot, right: &Snapshot) -> Result<Self, String> {
        let directory = tempfile::Builder::new()
            .prefix("yyeditor-compare-")
            .tempdir()
            .map_err(|e| e.to_string())?;
        // Text suffixes prevent Office/image detection based on an original name.
        let inputs = Self {
            left: directory.path().join("left.txt"),
            right: directory.path().join("right.txt"),
            directory,
        };
        write_snapshot(&inputs.left, left).map_err(|e| e.to_string())?;
        write_snapshot(&inputs.right, right).map_err(|e| e.to_string())?;
        Ok(inputs)
    }

    fn command(&self, executable: &Path, left_name: &str, right_name: &str) -> Command {
        let mut command = Command::new(executable);
        command
            .args(["--encoding", "UTF-8", "--left"])
            .arg(&self.left)
            .arg("--right")
            .arg(&self.right)
            .arg("--left-label")
            .arg(left_name)
            .arg("--right-label")
            .arg(right_name);
        command
    }

    fn run(self, command: Command) -> Result<ExitStatus, String> {
        self.run_until(command, None)
    }

    fn run_until(
        self,
        mut command: Command,
        deadline: Option<std::time::Instant>,
    ) -> Result<ExitStatus, String> {
        let mut child = command
            .spawn()
            .map_err(|e| format!("yymergeを起動できません: {e}"))?;
        let result = if let Some(deadline) = deadline {
            loop {
                match child.try_wait() {
                    Ok(Some(status)) => break Ok(status),
                    Err(error) => break Err(error),
                    Ok(None) if std::time::Instant::now() >= deadline => {
                        break Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "yymerge UI smoke timed out",
                        ));
                    }
                    Ok(None) => std::thread::sleep(std::time::Duration::from_millis(20)),
                }
            }
        } else {
            child.wait()
        };
        let status = match result {
            Ok(status) => status,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("yymergeの終了を確認できません: {error}"));
            }
        };
        // Keep inputs alive until the child releases them, even when the window
        // is still loading. Never remove source documents or Git objects.
        self.directory
            .close()
            .map_err(|e| format!("比較用一時ファイルを削除できません: {e}"))?;
        Ok(status)
    }
}

pub(crate) fn show(
    _owner: HWND,
    left_name: &str,
    left: &Snapshot,
    right_name: &str,
    right: &Snapshot,
) -> Result<(), String> {
    let executable = std::env::current_exe()
        .map_err(|e| e.to_string())?
        .with_file_name("yymergew.exe");
    if !executable.is_file() {
        return Err("yymergew.exeが見つかりません。yyeditor.exeと同じフォルダーに統合配布版のyymergew.exeを配置してください。".into());
    }
    let (left, right) = (left.clone(), right.clone());
    let (left_name, right_name) = (left_name.to_owned(), right_name.to_owned());
    std::thread::Builder::new()
        .name("yymerge-compare".into())
        .spawn(move || {
            let result = Inputs::new(&left, &right).and_then(|inputs| {
                let command = inputs.command(&executable, &left_name, &right_name);
                let status = inputs.run(command)?;
                // Differences (1) and user cancellation (130) are not failures.
                if matches!(status.code(), Some(0 | 1 | 130)) {
                    Ok(())
                } else {
                    Err(format!(
                        "yymergeの比較が完了しませんでした（終了コード: {status}）。"
                    ))
                }
            });
            if let Err(error) = result {
                // The editor frame may already have been closed. Do not use a stale
                // HWND as owner, or carry it across threads.
                unsafe {
                    MessageBoxW(
                        None,
                        &HSTRING::from(error),
                        &HSTRING::from("yyeditor — yymerge比較"),
                        MB_OK | MB_ICONERROR,
                    );
                }
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires a built yymergew.exe in YYMERGE_TEST_EXE and a Windows desktop"]
    fn real_yymerge_displays_editor_snapshots() {
        let executable = std::env::var_os("YYMERGE_TEST_EXE").expect("YYMERGE_TEST_EXE required");
        let report = tempfile::tempdir().unwrap();
        let inputs = Inputs::new(
            &Snapshot::from_bytes("\u{feff}未保存\r\n日本語\n末尾".as_bytes()),
            &Snapshot::empty(),
        )
        .unwrap();
        let directory = inputs.directory.path().to_owned();
        let mut command = inputs.command(
            Path::new(&executable),
            "--未保存 \"編集中\"",
            "HEAD（存在なし）",
        );
        command.arg("--ui-smoke-test").arg(report.path());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        assert_eq!(
            inputs.run_until(command, Some(deadline)).unwrap().code(),
            Some(0)
        );
        let text =
            std::fs::read_to_string(report.path().join("editor-comparison-smoke.txt")).unwrap();
        assert!(text.contains("LEFT: --未保存 \"編集中\" [読取専用]"));
        assert!(text.contains("\u{feff}未保存\r\n日本語\n末尾"));
        assert!(text.contains("RIGHT: HEAD（存在なし） [読取専用]"));
        assert!(!directory.exists());
    }

    #[test]
    fn immutable_inputs_preserve_unicode_eol_and_empty_git_side() {
        let bytes = "未保存\r\n二行目\n末尾".as_bytes();
        let original = Snapshot::from_bytes(bytes);
        let source = original.insert(0, "\u{feff}".as_bytes());
        let inputs = Inputs::new(&source, &Snapshot::empty()).unwrap();
        let staged = std::fs::read(&inputs.left).unwrap();
        assert_eq!(&staged[..6], b"\xEF\xBB\xBF\xEF\xBB\xBF");
        assert_eq!(&staged[6..], bytes);
        assert_eq!(original.len(), bytes.len() as u64);
        assert_eq!(std::fs::read(&inputs.right).unwrap(), b"\xEF\xBB\xBF");
        let directory = inputs.directory.path().to_owned();
        let command = inputs.command(Path::new("yymergew.exe"), "--日本語 \"HEAD\"", "変更中.txt");
        let args: Vec<_> = command
            .get_args()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args[7], "--日本語 \"HEAD\"");
        assert_eq!(args[9], "変更中.txt");
        drop(inputs);
        assert!(!directory.exists());
    }

    #[test]
    fn child_can_read_inputs_and_cleanup_follows_exit() {
        let inputs = Inputs::new(&Snapshot::from_bytes(b"unsaved"), &Snapshot::empty()).unwrap();
        let directory = inputs.directory.path().to_owned();
        // Use a real process to read inputs before returning a nonzero status.
        let mut command = Command::new("powershell.exe");
        command.args(["-NoProfile", "-NonInteractive", "-Command", "if ([IO.File]::ReadAllText($env:YY_TEST_COMPARE_LEFT) -ne 'unsaved') { exit 7 }; exit 1"])
            .env("YY_TEST_COMPARE_LEFT", &inputs.left);
        assert_eq!(inputs.run(command).unwrap().code(), Some(1));
        assert!(!directory.exists());
    }

    #[test]
    fn spawn_failure_cleans_up_and_large_snapshots_export() {
        let inputs = Inputs::new(&Snapshot::empty(), &Snapshot::empty()).unwrap();
        let directory = inputs.directory.path().to_owned();
        assert!(
            inputs
                .run(Command::new(directory.join("missing.exe")))
                .is_err()
        );
        assert!(!directory.exists());
        let big = Snapshot::from_bytes(vec![b'x'; 33 * 1024 * 1024]);
        let exported = Inputs::new(&big, &Snapshot::empty()).unwrap();
        assert_eq!(
            std::fs::metadata(&exported.left).unwrap().len(),
            big.len() + 3
        );
    }
}
