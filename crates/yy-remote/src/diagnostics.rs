//! Opt-in structured SSH diagnostics. This API deliberately accepts no arbitrary log text.
//! Do not copy credentials, commands, packet bodies, server prompts, paths or existing raw logs.
use std::{
    collections::VecDeque,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};

const MAX_EVENTS: usize = 8192;
const FILE_LIMIT: u64 = 4 << 20;
static NEXT: AtomicU64 = AtomicU64::new(1);
static CURRENT: OnceLock<Mutex<Option<Arc<Recorder>>>> = OnceLock::new();
fn current() -> &'static Mutex<Option<Arc<Recorder>>> {
    CURRENT.get_or_init(|| Mutex::new(None))
}
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Metadata values must come from program constants, counts or flags, never from remote/user text.
#[derive(Clone, Copy)]
pub enum Value {
    Code(&'static str),
    Number(u64),
    Flag(bool),
}
pub struct Recorder {
    start: Instant,
    path: PathBuf,
    state: Mutex<State>,
}
struct State {
    active: bool,
    file: std::fs::File,
    written: u64,
    events: VecDeque<String>,
    omitted: u64,
    write_failed: bool,
}

/// Anonymous connection correlation ID; no hostname or username is retained.
#[derive(Clone, Copy, Default)]
pub struct Context(pub u64);
impl Context {
    pub fn new() -> Self {
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
    pub fn event(
        self,
        stage: &'static str,
        outcome: &'static str,
        fields: &[(&'static str, Value)],
    ) {
        let recorder = lock(current()).clone();
        if let Some(recorder) = recorder {
            recorder.record(self.0, stage, outcome, fields);
        }
    }
    pub fn span(self, stage: &'static str) -> Span {
        let recorder = lock(current()).clone().filter(|r| lock(&r.state).active);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        if let Some(r) = &recorder {
            r.record(self.0, stage, "begin", &[("operation", Value::Number(id))]);
        }
        Span {
            recorder,
            context: self.0,
            id,
            stage,
            done: false,
        }
    }
}

pub struct Span {
    recorder: Option<Arc<Recorder>>,
    context: u64,
    id: u64,
    stage: &'static str,
    done: bool,
}
impl Span {
    pub fn id(&self) -> u64 {
        self.id
    }
    pub fn metadata(&self, fields: &[(&'static str, Value)]) {
        if let Some(r) = &self.recorder {
            let mut all = vec![("operation", Value::Number(self.id))];
            all.extend_from_slice(fields);
            r.record(self.context, self.stage, "metadata", &all);
        }
    }
    pub fn finish<T>(&mut self, result: &io::Result<T>) {
        if let Some(r) = &self.recorder {
            let mut fields = vec![("operation", Value::Number(self.id))];
            let outcome = match result {
                Ok(_) => "ok",
                Err(e) => {
                    fields.push(("error", Value::Code(error_kind(e.kind()))));
                    if let Some(code) = e.raw_os_error() {
                        fields.push(("os_error_unsigned", Value::Number(code as u32 as u64)));
                    }
                    "failed"
                }
            };
            r.record(self.context, self.stage, outcome, &fields);
        }
        self.done = true;
    }
    pub fn run<T>(mut self, operation: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
        let result = operation();
        self.finish(&result);
        result
    }
}
impl Drop for Span {
    fn drop(&mut self) {
        if !self.done
            && let Some(r) = &self.recorder
        {
            r.record(
                self.context,
                self.stage,
                "incomplete",
                &[("operation", Value::Number(self.id))],
            );
        }
    }
}

fn error_kind(kind: io::ErrorKind) -> &'static str {
    match kind {
        io::ErrorKind::PermissionDenied => "permission_denied",
        io::ErrorKind::TimedOut => "timed_out",
        io::ErrorKind::InvalidData => "invalid_data",
        io::ErrorKind::InvalidInput => "invalid_input",
        io::ErrorKind::UnexpectedEof => "unexpected_eof",
        io::ErrorKind::Interrupted => "cancelled",
        io::ErrorKind::ConnectionRefused => "connection_refused",
        io::ErrorKind::ConnectionReset => "connection_reset",
        io::ErrorKind::NotFound => "not_found",
        io::ErrorKind::Unsupported => "unsupported",
        io::ErrorKind::BrokenPipe => "broken_pipe",
        io::ErrorKind::NotConnected => "not_connected",
        _ => "other",
    }
}

impl Recorder {
    fn record(
        &self,
        context: u64,
        stage: &'static str,
        outcome: &'static str,
        fields: &[(&'static str, Value)],
    ) {
        let mut state = lock(&self.state);
        if !state.active {
            return;
        }
        let mut line = format!(
            "[{:>9.3}s] connection={context} stage={stage} outcome={outcome}",
            self.start.elapsed().as_secs_f64()
        );
        for (key, value) in fields.iter().take(16) {
            line.push_str(&format!(" {key}="));
            match value {
                Value::Code(v) => line.push_str(v),
                Value::Number(v) => line.push_str(&v.to_string()),
                Value::Flag(v) => line.push_str(if *v { "true" } else { "false" }),
            }
        }
        line.push('\n');
        if state.events.len() >= MAX_EVENTS || state.written + line.len() as u64 + 64 > FILE_LIMIT {
            state.omitted += 1;
            state.active = false;
            let _ = state
                .file
                .write_all(b"# capture_limit_reached; recording stopped\n");
            return;
        }
        if state
            .file
            .write_all(line.as_bytes())
            .and_then(|_| state.file.flush())
            .is_err()
        {
            state.write_failed = true;
        }
        state.written += line.len() as u64;
        state.events.push_back(line);
    }
}

/// Start a new local capture, preserving earlier files. An active capture is not reset.
pub fn start(directory: &Path, app: &'static str, version: &'static str) -> io::Result<PathBuf> {
    let mut slot = lock(current());
    if let Some(r) = slot.as_ref().filter(|r| lock(&r.state).active) {
        return Ok(r.path.clone());
    }
    std::fs::create_dir_all(directory)?;
    let mut file = tempfile::Builder::new()
        .prefix("ssh-diagnostic-")
        .suffix(".log")
        .tempfile_in(directory)?;
    let header = format!(
        "YY SSH diagnostic v1\napplication={app} version={version} os={} architecture={} pid={}\nstarted_utc={}\nPrivacy: no addresses, usernames, paths, commands, credentials, prompts, key bodies, file/terminal contents or raw packet bodies.\n",
        std::env::consts::OS,
        std::env::consts::ARCH,
        std::process::id(),
        crate::log::utc_clock()
    );
    file.write_all(header.as_bytes())?;
    let (file, path) = file.keep().map_err(|e| e.error)?;
    let recorder = Arc::new(Recorder {
        start: Instant::now(),
        path: path.clone(),
        state: Mutex::new(State {
            active: true,
            file,
            written: header.len() as u64,
            events: VecDeque::from([header]),
            omitted: 0,
            write_failed: false,
        }),
    });
    *slot = Some(recorder);
    Ok(path)
}
pub fn active() -> bool {
    lock(current())
        .as_ref()
        .is_some_and(|r| lock(&r.state).active)
}
pub fn path() -> Option<PathBuf> {
    lock(current()).as_ref().map(|r| r.path.clone())
}
pub fn stop() {
    let recorder = lock(current()).clone();
    if let Some(r) = recorder {
        r.record(0, "capture", "stopped", &[]);
        let mut s = lock(&r.state);
        s.active = false;
        let _ = s.file.flush();
    }
}
pub fn report() -> Option<String> {
    let recorder = lock(current()).clone()?;
    let state = lock(&recorder.state);
    let mut text = state.events.iter().cloned().collect::<String>();
    text.push_str(&format!(
        "\ncapture_active={} capture_limit_reached={} persistent_write_failed={}\n",
        state.active,
        state.omitted > 0,
        state.write_failed
    ));
    text.push_str("\n切り分け候補（推定。製品側ログとの照合が必要）\n");
    for (stage, hint) in [
        (
            "ssh.connect",
            "接続経路・ポート・鍵交換・ホスト鍵・認証の各イベントを確認してください。",
        ),
        (
            "ssh.auth",
            "ユーザー名の構成と認証方式、MFA、申請・接続許可を確認してください。",
        ),
        (
            "ssh.host_key",
            "登録済みホスト鍵との一致を管理者と確認してください。",
        ),
        (
            "ssh.channel_response",
            "SSHチャネル要求が拒否されています。exec／shell／SFTPの許可を確認してください。",
        ),
        (
            "ssh.channel",
            "exec／PTYなしshell／subsystemの許可設定を確認してください。",
        ),
        (
            "ssh.shell_ready",
            "ログインメニュー、PTY要否、開始前バナー、開始タイムアウトを確認してください。",
        ),
        (
            "sftp.handshake",
            "SFTPの許可と起動方法を確認してください。EssentialではSCPの選択が必要な場合があります。",
        ),
        (
            "scp.upload",
            "SCPと一覧・一時ファイル・権限操作用コマンドの許可を確認してください。",
        ),
        (
            "scp.download",
            "SCP起動要求、パス・権限、接続切断を確認してください。",
        ),
        (
            "directory.list",
            "一覧コマンドとlines／nul／ls形式、Unix出力のロケールや引用形式を確認してください。",
        ),
    ] {
        if state.events.iter().any(|line| {
            line.contains(&format!("stage={stage} "))
                && (line.contains("outcome=failed")
                    || line.contains("outcome=incomplete")
                    || line.contains("outcome=rejected"))
        }) {
            text.push_str(&format!("- {hint}\n"));
        }
    }
    text.push_str("\nこの診断はローカルクライアントの記録です。SecureCube側の監査ログ・設定を取得したものではありません。\n");
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn capture_is_opt_in_redacts_error_messages_and_keeps_order_and_limits() {
        // Only this test changes process-global capture state.
        let dir = tempfile::tempdir().unwrap();
        assert!(!active());
        let path = start(dir.path(), "test", "1").unwrap();
        let context = Context::new();
        let secret = "password=should-never-appear KEY-body sensitive-server-output";
        let result: io::Result<()> = context
            .span("ssh.auth")
            .run(|| Err(io::Error::new(io::ErrorKind::PermissionDenied, secret)));
        assert!(result.is_err());
        stop();
        let text = report().unwrap();
        assert!(text.contains("error=permission_denied"));
        assert!(!text.contains(secret));
        assert!(!std::fs::read_to_string(&path).unwrap().contains(secret));
        assert!(text.contains("認証方式"));
        assert!(!active());
        let next = start(dir.path(), "test", "1").unwrap();
        assert_ne!(next, path);
        for _ in 0..MAX_EVENTS + 10 {
            context.event("test", "count", &[]);
        }
        assert!(!active());
        assert!(report().unwrap().contains("capture_limit_reached=true"));
        assert!(std::fs::metadata(next).unwrap().len() <= FILE_LIMIT);
        *lock(current()) = None;
    }
}
