//! 実際の PTY で作成→デタッチ→再接続し、シェルの変数が残ることを確かめる。
//! Unix は手元、Windows は YY_TEST_WSL=<ディストリビューション名> の明示時だけ実行する。
//! python3 と、対象の tmux / screen がない環境ではそのテストをスキップする。

use std::io::{self, Read, Write};
use std::process::{ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use yy_remote::multiplex::{Binding, Kind, OpenMode, list_sessions};
use yy_remote::{Exit, Process, Shell, Transport, run, shell_quote};

const PTY: &str = r#"
import fcntl, os, pty, select, signal, struct, sys, termios
os.environ['TERM'] = sys.argv[2]
pid, master = pty.fork()
if pid == 0:
    fcntl.ioctl(0, termios.TIOCSWINSZ, struct.pack('HHHH', int(sys.argv[4]), int(sys.argv[3]), 0, 0))
    os.execl('/bin/sh', 'sh', '-c', sys.argv[1])
try:
    while True:
        ready, _, _ = select.select([0, master], [], [])
        if 0 in ready:
            data = os.read(0, 65536)
            if not data:
                try: os.kill(pid, signal.SIGHUP)
                except ProcessLookupError: pass
                break
            os.write(master, data)
        if master in ready:
            try: data = os.read(master, 65536)
            except OSError: break
            if not data: break
            os.write(1, data)
finally:
    os.close(master)
    os.waitpid(pid, 0)
"#;

struct Live;

impl Live {
    fn command(command: &[u8]) -> Command {
        #[cfg(windows)]
        let mut out = {
            let mut out = Command::new("wsl.exe");
            out.args([
                "-d",
                &std::env::var("YY_TEST_WSL").unwrap(),
                "--exec",
                "/bin/sh",
            ]);
            out
        };
        #[cfg(not(windows))]
        let mut out = Command::new("/bin/sh");
        out.arg("-c").arg(std::str::from_utf8(command).unwrap());
        out
    }

    fn available(kind: Kind) -> bool {
        if cfg!(windows) && std::env::var_os("YY_TEST_WSL").is_none() {
            eprintln!("skip live multiplexer test: YY_TEST_WSL is not set");
            return false;
        }
        let command = format!(
            "command -v python3 >/dev/null && command -v {} >/dev/null",
            kind.program()
        );
        let available = Self::command(command.as_bytes())
            .status()
            .is_ok_and(|s| s.success());
        if !available {
            eprintln!(
                "skip live {} test: program or python3 is unavailable",
                kind.program()
            );
        }
        available
    }
}

struct Input(Arc<Mutex<Option<ChildStdin>>>);

impl Write for Input {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .unwrap()
            .as_mut()
            .ok_or(io::ErrorKind::BrokenPipe)?
            .write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0
            .lock()
            .unwrap()
            .as_mut()
            .ok_or(io::ErrorKind::BrokenPipe)?
            .flush()
    }
}

impl Transport for Live {
    fn is_closed(&self) -> bool {
        false
    }

    fn exec(&self, command: &[u8]) -> io::Result<Process> {
        let mut child = Self::command(command)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let input = child.stdin.take().unwrap();
        let output = child.stdout.take().unwrap();
        Ok(Process::new(
            Box::new(input),
            Box::new(output),
            Box::new(move || {
                let out = child.wait_with_output()?;
                Ok(Exit {
                    status: out.status.code().map(|s| s as u32),
                    stderr: out.stderr,
                })
            }),
        ))
    }

    fn shell(&self, term: &str, size: (u16, u16), command: Option<&[u8]>) -> io::Result<Shell> {
        let mut cmd = b"python3 -u -c ".to_vec();
        cmd.extend(shell_quote(PTY.as_bytes()));
        cmd.push(b' ');
        cmd.extend(shell_quote(command.unwrap()));
        cmd.push(b' ');
        cmd.extend(shell_quote(term.as_bytes()));
        cmd.extend_from_slice(format!(" {} {}", size.0, size.1).as_bytes());
        let mut child = Self::command(&cmd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        let input = Arc::new(Mutex::new(child.stdin.take()));
        let close = input.clone();
        Ok(Shell {
            input: Box::new(Input(input)),
            output: Box::new(child.stdout.take().unwrap()),
            resize: Box::new(|_, _| {}),
            finish: Box::new(move || {
                Ok(Exit {
                    status: child.wait()?.code().map(|s| s as u32),
                    stderr: Vec::new(),
                })
            }),
            close: Box::new(move || {
                close.lock().unwrap().take();
            }),
        })
    }
}

struct Cleanup<'a>(&'a Live, Vec<u8>);
impl Drop for Cleanup<'_> {
    fn drop(&mut self) {
        let _ = run(self.0, &self.1, b"");
    }
}

// WSL の短い exec をチャネルの代わりに使う間も、SSH 接続に相当するプロセスを残す。
// 最後の wsl.exe が終了すると、WSL 側でデーモンまで停止する環境がある。
struct KeepAlive(std::process::Child);

impl Drop for KeepAlive {
    fn drop(&mut self) {
        self.0.stdin.take();
        let _ = self.0.wait();
    }
}

fn roundtrip(kind: Kind) {
    if !Live::available(kind) {
        return;
    }
    let _connection = KeepAlive(
        Live::command(b"cat >/dev/null")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let t = Live;
    let prefix = format!(
        "yyt-{}-{}-",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let binding = Binding::new(kind, &prefix, "test").unwrap();
    let name = binding.session_name();
    let cleanup = match kind {
        Kind::Tmux => format!("tmux kill-session -t '={name}'"),
        Kind::Screen => format!("screen -S '{name}' -X quit"),
    };
    let _cleanup = Cleanup(&t, cleanup.into_bytes());
    for reconnect in [false, true] {
        let binding = if reconnect {
            let sessions = list_sessions(&t, kind).unwrap();
            let existing = sessions
                .iter()
                .find(|s| s.name == name)
                .expect("created session must be listed after detach");
            Binding::from_session(kind, &prefix, &existing.name).unwrap()
        } else {
            binding.clone()
        };
        let mode = if reconnect {
            OpenMode::Attach
        } else {
            OpenMode::Create
        };
        let s = binding
            .open_mode(&t, "xterm-256color", (100, 30), None, mode)
            .unwrap();
        let Shell {
            mut input,
            mut output,
            finish,
            close,
            ..
        } = s;
        let text = Arc::new(Mutex::new(Vec::new()));
        let read_text = text.clone();
        let reader = std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = output.read(&mut buf) {
                if n == 0 {
                    break;
                }
                read_text.lock().unwrap().extend_from_slice(&buf[..n]);
            }
        });
        // 一致する文字列を入力のエコーに含めず、シェルが実行した出力を待つ。
        let marker = if reconnect {
            "yy-persistent-state"
        } else {
            "yy-ready"
        };
        let command = if reconnect {
            "printf '%s\\n' \"$YY_TEST_STATE\"\n"
        } else {
            "export YY_TEST_STATE=yy-persistent-state; printf '%s%s\\n' 'yy-' 'ready'\n"
        };
        input.write_all(command.as_bytes()).unwrap();
        input.flush().unwrap();
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(15) {
            if text
                .lock()
                .unwrap()
                .windows(marker.len())
                .any(|w| w == marker.as_bytes())
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        // open_mode は PTY の開始を返す。シェルの応答後ならセッションも作成済み。
        let ready = text
            .lock()
            .unwrap()
            .windows(marker.len())
            .any(|w| w == marker.as_bytes());
        let collision = if ready {
            Some(binding.open_mode(&t, "xterm-256color", (100, 30), None, OpenMode::Create))
        } else {
            None
        };
        close();
        drop(input);
        finish().unwrap();
        reader.join().unwrap();
        let text = text.lock().unwrap();
        assert!(
            text.windows(marker.len()).any(|w| w == marker.as_bytes()),
            "{}: {}",
            kind.program(),
            String::from_utf8_lossy(&text)
        );
        let collision = collision
            .unwrap()
            .err()
            .expect("new session must not reuse an existing one");
        assert_eq!(collision.kind(), io::ErrorKind::AlreadyExists);
    }
}

#[test]
fn tmux_session_survives_detach_and_reconnect() {
    roundtrip(Kind::Tmux);
}

#[test]
fn screen_session_survives_detach_and_reconnect() {
    roundtrip(Kind::Screen);
}
