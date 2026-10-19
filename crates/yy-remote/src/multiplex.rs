//! ターミナルのタブと tmux / GNU Screen の永続セッションを結び付ける。
//! タブを閉じると SSH のクライアント側チャネルだけが閉じる。セッションへ exit・quit を送らない。

use std::io;

use crate::{Shell, Transport, run, shell_quote};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Tmux,
    Screen,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenMode {
    CreateOrAttach,
    Create,
    Attach,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionInfo {
    pub name: String,
    pub attached: bool,
}

/// プログラムがない・一覧を読めない場合はエラー。セッションがない場合は空の一覧。
pub fn list_sessions(t: &dyn Transport, kind: Kind) -> io::Result<Vec<SessionInfo>> {
    check_program(t, kind)?;
    let command: &[u8] = match kind {
        // tmux は出力時にタブをエスケープする。セッション名に使えない : で区切る。
        Kind::Tmux => b"LC_ALL=C tmux list-sessions -F '#{session_name}:#{session_attached}'",
        Kind::Screen => b"LC_ALL=C screen -ls",
    };
    let out = run(t, &script(command), b"")?;
    if !out.success() {
        let message = out.message();
        let empty = match kind {
            Kind::Tmux => {
                message.contains("no server running")
                    || message.contains("No such file or directory")
            }
            Kind::Screen => {
                message.contains("No Sockets found") || message.contains("No screen session found")
            }
        };
        if !empty {
            return Err(io::Error::other(format!(
                "{} のセッション一覧を読めません: {message}",
                kind.program()
            )));
        }
    }
    let mut sessions = match kind {
        Kind::Tmux => String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|line| {
                let (name, attached) = line.rsplit_once(':')?;
                let attached: u32 = attached.parse().ok()?;
                Some(SessionInfo {
                    name: name.to_owned(),
                    attached: attached > 0,
                })
            })
            .collect::<Vec<_>>(),
        Kind::Screen => screen_entries(&out.stdout)
            .into_iter()
            .map(|(_, name, attached)| SessionInfo { name, attached })
            .collect(),
    };
    sessions.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(sessions)
}

fn check_program(t: &dyn Transport, kind: Kind) -> io::Result<()> {
    let program = kind.program();
    checked(
        t,
        format!("command -v {program} >/dev/null 2>&1").as_bytes(),
        &format!("接続先に {program} がありません（PATH を確認してください）"),
    )
}

impl Kind {
    pub fn program(self) -> &'static str {
        match self {
            Self::Tmux => "tmux",
            Self::Screen => "screen",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    pub kind: Kind,
    tab_name: String,
    session_name: String,
}

fn component(value: &str, label: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err(format!("{label}は必須です"));
    }
    if !value
        .chars()
        .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_'))
    {
        return Err(format!(
            "{label}には文字・数字・ハイフン・アンダースコアを使ってください（空白・ピリオド・コロンは使えません）"
        ));
    }
    Ok(())
}

pub fn validate_prefix(prefix: &str) -> Result<(), String> {
    component(prefix, "セッション名のプレフィクス")?;
    if prefix.len() >= 64 {
        return Err("プレフィクスとタブ名の合計を UTF-8 で 64 バイト以内にしてください".into());
    }
    Ok(())
}

fn quote(value: &str) -> String {
    String::from_utf8(shell_quote(value.as_bytes())).expect("UTF-8 quoting")
}

// 接続先のログインシェルが csh / fish でも、組み立てたコマンドは POSIX sh で実行する。
fn script(command: &[u8]) -> Vec<u8> {
    let mut out = b"exec sh -c ".to_vec();
    out.extend(shell_quote(command));
    out
}

fn checked(t: &dyn Transport, command: &[u8], what: &str) -> io::Result<()> {
    let out = run(t, &script(command), b"")?;
    if !out.success() {
        return Err(io::Error::other(format!("{what}: {}", out.message())));
    }
    Ok(())
}

impl Binding {
    pub fn new(kind: Kind, prefix: &str, tab_name: &str) -> Result<Self, String> {
        validate_prefix(prefix)?;
        component(tab_name, "タブ名")?;
        let session_name = format!("{prefix}{tab_name}");
        if session_name.len() > 64 {
            return Err("プレフィクスとタブ名の合計を UTF-8 で 64 バイト以内にしてください".into());
        }
        Ok(Self {
            kind,
            tab_name: tab_name.into(),
            session_name,
        })
    }

    pub fn tab_name(&self) -> &str {
        &self.tab_name
    }

    pub fn session_name(&self) -> &str {
        &self.session_name
    }

    pub fn from_session(kind: Kind, prefix: &str, session: &str) -> Result<Self, String> {
        let tab = session
            .strip_prefix(prefix)
            .ok_or("登録済みプレフィクスと一致しません")?;
        Self::new(kind, prefix, tab)
    }

    /// 新規作成または既存セッションへ接続する。選んだプログラムがない場合は明示的に失敗する。
    /// `Shell::close` はクライアントの切断なので、どのタブの終了経路でもデタッチになる。
    pub fn open(
        &self,
        t: &dyn Transport,
        term: &str,
        size: (u16, u16),
        dir: Option<&[u8]>,
    ) -> io::Result<Shell> {
        self.open_mode(t, term, size, dir, OpenMode::CreateOrAttach)
    }

    pub fn open_mode(
        &self,
        t: &dyn Transport,
        term: &str,
        size: (u16, u16),
        dir: Option<&[u8]>,
        mode: OpenMode,
    ) -> io::Result<Shell> {
        check_program(t, self.kind)?;
        let mut cwd = Vec::new();
        if let Some(dir) = dir {
            cwd.extend_from_slice(b"cd ");
            cwd.extend(shell_quote(dir));
            cwd.extend_from_slice(b" && ");
        }
        let command = match self.kind {
            Kind::Tmux => {
                if mode != OpenMode::CreateOrAttach {
                    let out = run(
                        t,
                        &script(
                            format!(
                                "tmux has-session -t {}",
                                quote(&format!("={}", self.session_name))
                            )
                            .as_bytes(),
                        ),
                        b"",
                    )?;
                    if mode == OpenMode::Create && out.success() {
                        return Err(io::Error::new(
                            io::ErrorKind::AlreadyExists,
                            "同名のセッションがすでにあります。新しいタブ名を指定してください",
                        ));
                    }
                    if mode == OpenMode::Attach && !out.success() {
                        return Err(io::Error::new(
                            io::ErrorKind::NotFound,
                            "選んだセッションは終了しています。一覧を開き直してください",
                        ));
                    }
                }
                // -A は同名があれば接続、なければ作成。既存クライアントを追い出さない。
                // セッションを孤立時に削除する設定を無効にし、切断後も残す。
                let start = match mode {
                    OpenMode::CreateOrAttach => {
                        format!("new-session -A -s {}", quote(&self.session_name))
                    }
                    OpenMode::Create => format!("new-session -s {}", quote(&self.session_name)),
                    OpenMode::Attach => format!(
                        "attach-session -t {}",
                        quote(&format!("={}", self.session_name))
                    ),
                };
                cwd.extend_from_slice(format!(
                    "exec tmux {start} \\; set-option -t {} destroy-unattached off \\; set-option -s exit-unattached off",
                    quote(&format!("={}", self.session_name)),
                ).as_bytes());
                script(&cwd)
            }
            Kind::Screen => {
                // screen の -S / -r は部分一致もする。完全に同じ名前の PID.name を選んでから接続する。
                let id = self.screen_id(t)?;
                if mode == OpenMode::Create && id.is_some() {
                    return Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        "同名のセッションがすでにあります。新しいタブ名を指定してください",
                    ));
                }
                if mode == OpenMode::Attach && id.is_none() {
                    return Err(io::Error::new(
                        io::ErrorKind::NotFound,
                        "選んだセッションは終了しています。一覧を開き直してください",
                    ));
                }
                if id.is_none() {
                    cwd.extend_from_slice(
                        format!(
                            "exec screen -m -S {} \"${{SHELL:-/bin/sh}}\" -l",
                            quote(&self.session_name),
                        )
                        .as_bytes(),
                    );
                    // 最初からタブの PTY で作成する。短命な exec チャネルで作ってから
                    // つなぎ直すと、その終了時に screen が停止する接続環境がある。
                    let shell = t.shell(term, size, Some(&script(&cwd)))?;
                    let configure = (|| {
                        for _ in 0..20 {
                            if let Some(id) = self.screen_id(t)? {
                                return self.screen_autodetach(t, &id);
                            }
                            std::thread::sleep(std::time::Duration::from_millis(50));
                        }
                        Err(io::Error::other(
                            "作成した screen セッションが見つかりません",
                        ))
                    })();
                    if let Err(e) = configure {
                        (shell.close)();
                        return Err(e);
                    }
                    return Ok(shell);
                }
                let id = id.unwrap();
                self.screen_autodetach(t, &id)?;
                // -x はほかの接続をデタッチせず、既存の同一セッションに接続する。
                script(format!("exec screen -x {}", quote(&id)).as_bytes())
            }
        };
        t.shell(term, size, Some(&command))
    }

    fn screen_id(&self, t: &dyn Transport) -> io::Result<Option<String>> {
        let out = run(t, &script(b"LC_ALL=C screen -ls"), b"")?;
        screen_id(&out.stdout, &self.session_name)
    }

    fn screen_autodetach(&self, t: &dyn Transport, id: &str) -> io::Result<()> {
        checked(
            t,
            format!("screen -S {} -X autodetach on", quote(id)).as_bytes(),
            "screen の自動デタッチを設定できません",
        )
    }
}

fn screen_entries(output: &[u8]) -> Vec<(String, String, bool)> {
    let text = String::from_utf8_lossy(output);
    let mut entries = Vec::new();
    for line in text.lines() {
        if line.to_ascii_lowercase().contains("(dead") {
            continue;
        }
        let Some(id) = line.split_whitespace().next() else {
            continue;
        };
        let Some((pid, session)) = id.split_once('.') else {
            continue;
        };
        if pid.is_empty() || !pid.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let state = line.to_ascii_lowercase();
        entries.push((
            id.to_owned(),
            session.to_owned(),
            state.contains("attached") && !state.contains("detached"),
        ));
    }
    entries
}

fn screen_id(output: &[u8], name: &str) -> io::Result<Option<String>> {
    let mut found = None;
    for (id, session, _) in screen_entries(output) {
        if session != name {
            continue;
        }
        if found.is_some() {
            return Err(io::Error::other(format!(
                "screen に同名のセッション {name} が複数あります。接続先で重複を解消してください"
            )));
        }
        found = Some(id);
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Exit, Output, Process};
    use std::collections::VecDeque;
    use std::io::Cursor;
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    #[derive(Default)]
    struct Fake {
        answers: Mutex<VecDeque<Output>>,
        commands: Mutex<Vec<Vec<u8>>>,
        shells: Mutex<Vec<Vec<u8>>>,
        closed: Arc<AtomicUsize>,
    }

    impl Fake {
        fn with(answers: Vec<Output>) -> Self {
            Self {
                answers: Mutex::new(answers.into()),
                ..Default::default()
            }
        }
    }

    impl Transport for Fake {
        fn exec(&self, command: &[u8]) -> io::Result<Process> {
            self.commands.lock().unwrap().push(command.to_vec());
            let out = self
                .answers
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected remote command");
            Ok(Process::new(
                Box::new(io::sink()),
                Box::new(Cursor::new(out.stdout)),
                Box::new(move || {
                    Ok(Exit {
                        status: out.status,
                        stderr: out.stderr,
                    })
                }),
            ))
        }

        fn is_closed(&self) -> bool {
            false
        }

        fn shell(&self, term: &str, size: (u16, u16), command: Option<&[u8]>) -> io::Result<Shell> {
            assert_eq!((term, size), ("xterm-256color", (100, 30)));
            self.shells.lock().unwrap().push(command.unwrap().to_vec());
            let closed = self.closed.clone();
            Ok(Shell {
                input: Box::new(io::sink()),
                output: Box::new(io::empty()),
                resize: Box::new(|_, _| {}),
                finish: Box::new(|| Ok(Exit::default())),
                close: Box::new(move || {
                    closed.fetch_add(1, Ordering::SeqCst);
                }),
            })
        }
    }

    fn ok(stdout: &[u8]) -> Output {
        Output {
            status: Some(0),
            stdout: stdout.to_vec(),
            stderr: Vec::new(),
        }
    }

    #[test]
    fn names_require_both_parts_and_preserve_the_exact_concatenation() {
        for kind in [Kind::Tmux, Kind::Screen] {
            let binding = Binding::new(kind, "work-", "開発").unwrap();
            assert_eq!(binding.tab_name(), "開発");
            assert_eq!(binding.session_name(), "work-開発");
            for (prefix, tab) in [
                ("", "tab"),
                ("p-", ""),
                (" ", "tab"),
                ("p-", "a:b"),
                ("p-", "a.b"),
                ("p-", "$(id)"),
                ("p-", "a\nb"),
            ] {
                assert!(Binding::new(kind, prefix, tab).is_err());
            }
            assert!(Binding::new(kind, &"a".repeat(64), "tab").is_err());
            assert!(Binding::new(kind, "p-", &"開".repeat(22)).is_err());
        }
    }

    #[test]
    fn lists_sessions_with_names_states_and_dead_screen_filtering() {
        let t = Fake::with(vec![ok(b""), ok(b"work-z:0\nwork-a:2\n")]);
        let sessions = list_sessions(&t, Kind::Tmux).unwrap();
        assert_eq!(
            sessions,
            vec![
                SessionInfo {
                    name: "work-a".into(),
                    attached: true
                },
                SessionInfo {
                    name: "work-z".into(),
                    attached: false
                }
            ]
        );
        let t = Fake::with(vec![ok(b""), ok(b"There are screens on:\n 11.work-z (Detached)\n 22.work-a (Multi, attached)\n 33.work-dead (Dead ???)\n")]);
        assert_eq!(list_sessions(&t, Kind::Screen).unwrap(), sessions);
        let restored = Binding::from_session(Kind::Tmux, "work-", &sessions[0].name).unwrap();
        assert_eq!(restored.tab_name(), "a");
        assert_eq!(restored.session_name(), "work-a");
        assert!(Binding::from_session(Kind::Tmux, "other-", "work-a").is_err());
    }

    #[test]
    fn empty_list_is_distinct_from_failed_listing() {
        for (kind, message) in [
            (Kind::Tmux, "no server running on /tmp/tmux"),
            (Kind::Screen, "No Sockets found in /run/screen"),
        ] {
            let t = Fake::with(vec![
                ok(b""),
                Output {
                    status: Some(1),
                    stderr: message.as_bytes().to_vec(),
                    ..Default::default()
                },
            ]);
            assert!(list_sessions(&t, kind).unwrap().is_empty());
            let t = Fake::with(vec![
                ok(b""),
                Output {
                    status: Some(1),
                    stderr: b"Permission denied".to_vec(),
                    ..Default::default()
                },
            ]);
            assert!(list_sessions(&t, kind).is_err());
        }
    }

    #[test]
    fn new_session_never_reuses_an_existing_one_and_attach_never_creates() {
        for kind in [Kind::Tmux, Kind::Screen] {
            let binding = Binding::new(kind, "p-", "build").unwrap();
            let existing = if kind == Kind::Tmux {
                ok(b"")
            } else {
                ok(b"11.p-build (Detached)\n")
            };
            let t = Fake::with(vec![ok(b""), existing]);
            let err = binding
                .open_mode(&t, "xterm-256color", (100, 30), None, OpenMode::Create)
                .err()
                .unwrap();
            assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
            assert!(t.shells.lock().unwrap().is_empty());
            let absent = if kind == Kind::Tmux {
                Output {
                    status: Some(1),
                    ..Default::default()
                }
            } else {
                ok(b"")
            };
            let t = Fake::with(vec![ok(b""), absent]);
            let err = binding
                .open_mode(&t, "xterm-256color", (100, 30), None, OpenMode::Attach)
                .err()
                .unwrap();
            assert_eq!(err.kind(), io::ErrorKind::NotFound);
            assert!(t.shells.lock().unwrap().is_empty());
        }
    }

    #[test]
    fn tmux_creates_or_attaches_and_closing_only_disconnects_the_client() {
        let t = Fake::with(vec![ok(b"")]);
        let binding = Binding::new(Kind::Tmux, "p-", "build").unwrap();
        let s = binding
            .open(&t, "xterm-256color", (100, 30), Some(b"/tmp/a' $(unsafe)"))
            .unwrap();
        let commands_before = t.commands.lock().unwrap().clone();
        let shells = t.shells.lock().unwrap();
        let command = String::from_utf8_lossy(&shells[0]);
        assert!(command.contains("new-session -A -s"));
        assert!(command.contains("p-build"));
        assert!(command.contains("=p-build"));
        assert!(command.contains("destroy-unattached off"));
        assert!(command.contains("exit-unattached off"));
        assert!(command.starts_with("exec sh -c "));
        // 作業フォルダは 2 段のシェル引用を通しても 1 つの引数として渡す。
        let mut inner = b"cd ".to_vec();
        inner.extend(shell_quote(b"/tmp/a' $(unsafe)"));
        inner.extend_from_slice(b" && ");
        assert!(
            shells[0]
                .windows(shell_quote(&inner).len() - 2)
                .any(|w| w == &shell_quote(&inner)[1..shell_quote(&inner).len() - 1])
        );
        (s.close)();
        assert_eq!(t.closed.load(Ordering::SeqCst), 1);
        assert_eq!(*t.commands.lock().unwrap(), commands_before);
    }

    #[test]
    fn screen_reconnects_to_the_exact_session_and_enables_autodetach() {
        let t = Fake::with(vec![
            ok(b""),
            ok(b"\t11.p-build-old (Detached)\n\t22.p-build (Attached)\n"),
            ok(b""),
        ]);
        let s = Binding::new(Kind::Screen, "p-", "build")
            .unwrap()
            .open(&t, "xterm-256color", (100, 30), None)
            .unwrap();
        let commands = t.commands.lock().unwrap();
        assert_eq!(commands.len(), 3);
        let setting = String::from_utf8_lossy(&commands[2]);
        assert!(setting.contains("22.p-build"));
        assert!(setting.contains("-X autodetach on"));
        let shells = t.shells.lock().unwrap();
        let attach = String::from_utf8_lossy(&shells[0]);
        assert!(attach.contains("exec screen -x"));
        assert!(attach.contains("22.p-build"));
        (s.close)();
        assert_eq!(t.closed.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn screen_creates_when_only_a_similarly_named_session_exists() {
        let t = Fake::with(vec![
            ok(b""),
            ok(b"11.p-build2 (Detached)\n12.p-build (Dead ???)\n"),
            ok(b""),
            ok(b"11.p-build2 (Detached)\n22.p-build (Detached)\n"),
            ok(b""),
        ]);
        Binding::new(Kind::Screen, "p-", "build")
            .unwrap()
            .open(&t, "xterm-256color", (100, 30), Some(b"/work dir"))
            .unwrap();
        let shells = t.shells.lock().unwrap();
        assert!(String::from_utf8_lossy(&shells[0]).contains("exec screen -m -S"));
        assert!(String::from_utf8_lossy(&shells[0]).contains("/work dir"));
        assert!(t.answers.lock().unwrap().is_empty());
    }

    #[test]
    fn absent_program_or_ambiguous_screen_never_falls_back_to_a_plain_shell() {
        for kind in [Kind::Tmux, Kind::Screen] {
            let t = Fake::with(vec![Output {
                status: Some(127),
                ..Default::default()
            }]);
            let err = Binding::new(kind, "p-", "build")
                .unwrap()
                .open(&t, "xterm-256color", (100, 30), None)
                .err()
                .unwrap();
            assert!(err.to_string().contains(kind.program()));
            assert!(t.shells.lock().unwrap().is_empty());
        }
        let t = Fake::with(vec![
            ok(b""),
            ok(b"11.p-build (Detached)\n22.p-build (Detached)\n"),
        ]);
        assert!(
            Binding::new(Kind::Screen, "p-", "build")
                .unwrap()
                .open(&t, "xterm-256color", (100, 30), None)
                .is_err()
        );
        assert!(t.shells.lock().unwrap().is_empty());
    }
}
