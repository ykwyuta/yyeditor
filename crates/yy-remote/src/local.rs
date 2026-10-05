//! SSH を使わずに手元で `sh -c` を起動する [`Transport`]（テスト用）。
//!
//! エージェントの配置から保存までの流れを、SSH サーバーなしで確かめるために使う。

use std::io::{self, Read};
use std::os::unix::ffi::OsStrExt;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::{Exit, Process, STDERR_LIMIT, Transport};

#[derive(Default)]
pub struct LocalTransport {
    closed: AtomicBool,
}

impl LocalTransport {
    pub fn new() -> LocalTransport {
        LocalTransport::default()
    }

    /// 接続が切れた状態にする（以後の起動は失敗する）。
    pub fn close(&self) {
        self.closed.store(true, Ordering::Relaxed);
    }
}

impl Transport for LocalTransport {
    fn exec(&self, command: &[u8]) -> io::Result<Process> {
        if self.is_closed() {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "接続が切れています",
            ));
        }
        let mut child = Command::new("sh")
            .arg("-c")
            .arg(std::ffi::OsStr::from_bytes(command))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let stdin = child.stdin.take().expect("piped");
        let stdout = child.stdout.take().expect("piped");
        let mut stderr = child.stderr.take().expect("piped");
        let errors = std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = (&mut stderr)
                .take(STDERR_LIMIT as u64)
                .read_to_end(&mut buf);
            let _ = io::copy(&mut stderr, &mut io::sink());
            buf
        });
        let finish = Box::new(move || {
            let status = child.wait()?;
            let stderr = errors.join().unwrap_or_default();
            Ok(Exit {
                status: status.code().map(|c| c as u32),
                stderr,
            })
        });
        Ok(Process::new(Box::new(stdin), Box::new(stdout), finish))
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }
}
