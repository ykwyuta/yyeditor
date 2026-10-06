//! 組み込みの SSH クライアント（yy-ssh）の結合テスト。
//!
//! russh のサーバーを同じプロセスで立て（exec 要求は手元の `sh -c` で実行）、OpenSSH を使わずに
//! ホスト鍵の確認・認証・コマンド実行・エージェントの配置と保存、踏み台（direct-tcpip）と
//! プロキシ（HTTP CONNECT・SOCKS5）を経由した接続までを確かめる。
#![cfg(unix)]

use std::collections::HashMap;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use russh::keys::{Algorithm, PrivateKey, PublicKey};
use russh::server::{self, Auth, Msg, Session};
use russh::{Channel, ChannelId, ChannelMsg, ChannelOpenFailure, MethodKind, MethodSet};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use yy_remote::proxy::Proxy;
use yy_remote::uri::Target;
use yy_remote::{
    AgentFiles, ConnectLog, Connector, HostKeyCheck, HostKeyQuestion, HostSpec, MemoryPasswords,
    PassphraseRequest, PasswordAnswer, PasswordRequest, PasswordStore, Prompter, SavedPassword,
    Session as Remote, UploadOutcome,
};
use yy_ssh::SshConnector;

const PASSWORD: &str = "correct horse";
const PASSPHRASE: &str = "pass phrase";

fn random_key() -> PrivateKey {
    PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap()
}

/// テスト用の SSH サーバー。
struct TestServer {
    port: u16,
    host_key: PublicKey,
    /// 認証の試行回数
    auth_attempts: Arc<AtomicUsize>,
    /// 踏み台として中継を頼まれた接続先
    forwards: Arc<Mutex<Vec<String>>>,
    /// 端末の要求（`pty xterm-256color 80x24`・`resize 100x30`）
    ptys: Arc<Mutex<Vec<String>>>,
    _rt: tokio::runtime::Runtime,
}

#[derive(Clone)]
struct Handler {
    allowed_keys: Arc<Vec<PublicKey>>,
    auth_attempts: Arc<AtomicUsize>,
    channels: Arc<Mutex<HashMap<ChannelId, Channel<Msg>>>>,
    forwards: Arc<Mutex<Vec<String>>>,
    ptys: Arc<Mutex<Vec<String>>>,
}

impl server::Handler for Handler {
    type Error = russh::Error;

    async fn auth_password(&mut self, _: &str, password: &str) -> Result<Auth, Self::Error> {
        self.auth_attempts.fetch_add(1, Ordering::SeqCst);
        // OpenSSH と同じく、間違えてもパスワードを入れ直せる
        Ok(if password == PASSWORD {
            Auth::Accept
        } else {
            Auth::Reject {
                proceed_with_methods: Some(MethodSet::from(
                    &[MethodKind::Password, MethodKind::PublicKey][..],
                )),
                partial_success: false,
            }
        })
    }

    async fn auth_publickey(&mut self, _: &str, key: &PublicKey) -> Result<Auth, Self::Error> {
        self.auth_attempts.fetch_add(1, Ordering::SeqCst);
        let ok = self
            .allowed_keys
            .iter()
            .any(|k| k.key_data() == key.key_data());
        Ok(if ok { Auth::Accept } else { Auth::reject() })
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: server::ChannelOpenHandle,
        _: &mut Session,
    ) -> Result<(), Self::Error> {
        self.channels.lock().unwrap().insert(channel.id(), channel);
        reply.accept().await;
        Ok(())
    }

    async fn channel_open_direct_tcpip(
        &mut self,
        channel: Channel<Msg>,
        host: &str,
        port: u32,
        _: &str,
        _: u32,
        reply: server::ChannelOpenHandle,
        _: &mut Session,
    ) -> Result<(), Self::Error> {
        self.forwards.lock().unwrap().push(format!("{host}:{port}"));
        match tokio::net::TcpStream::connect((host, port as u16)).await {
            Ok(mut tcp) => {
                reply.accept().await;
                tokio::spawn(async move {
                    let mut ch = channel.into_stream();
                    let _ = tokio::io::copy_bidirectional(&mut ch, &mut tcp).await;
                });
            }
            Err(_) => reply.reject(ChannelOpenFailure::ConnectFailed).await,
        }
        Ok(())
    }

    async fn pty_request(
        &mut self,
        id: ChannelId,
        term: &str,
        cols: u32,
        rows: u32,
        _: u32,
        _: u32,
        _: &[(russh::Pty, u32)],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.ptys
            .lock()
            .unwrap()
            .push(format!("pty {term} {cols}x{rows}"));
        session.channel_success(id)?;
        Ok(())
    }

    async fn window_change_request(
        &mut self,
        _: ChannelId,
        cols: u32,
        rows: u32,
        _: u32,
        _: u32,
        _: &mut Session,
    ) -> Result<(), Self::Error> {
        self.ptys
            .lock()
            .unwrap()
            .push(format!("resize {cols}x{rows}"));
        Ok(())
    }

    /// 対話シェル（端末はないので、標準入力からコマンドを読む `sh`）
    async fn shell_request(
        &mut self,
        id: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let Some(channel) = self.channels.lock().unwrap().remove(&id) else {
            return Ok(());
        };
        session.channel_success(id)?;
        tokio::spawn(run_command(channel, b"exec sh".to_vec()));
        Ok(())
    }

    /// `sftp` サブシステム（手元の sftp-server を動かす）
    async fn subsystem_request(
        &mut self,
        id: ChannelId,
        name: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let server = yy_remote::local::sftp_server();
        let Some(channel) = self.channels.lock().unwrap().remove(&id) else {
            return Ok(());
        };
        match (name, server) {
            ("sftp", Some(p)) => {
                session.channel_success(id)?;
                let cmd = format!("exec {}", p.display()).into_bytes();
                tokio::spawn(run_command(channel, cmd));
            }
            _ => session.channel_failure(id)?,
        }
        Ok(())
    }

    async fn exec_request(
        &mut self,
        id: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let Some(channel) = self.channels.lock().unwrap().remove(&id) else {
            return Ok(());
        };
        let command = data.to_vec();
        session.channel_success(id)?;
        tokio::spawn(run_command(channel, command));
        Ok(())
    }
}

/// exec 要求を `sh -c` で実行し、標準入出力をチャネルにつなぐ。
async fn run_command(channel: Channel<Msg>, command: Vec<u8>) {
    let mut child = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(std::ffi::OsStr::from_bytes(&command))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take();
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let (mut rd, wr) = channel.split();
    let input = tokio::spawn(async move {
        while let Some(msg) = rd.wait().await {
            match msg {
                ChannelMsg::Data { data } => {
                    if let Some(s) = stdin.as_mut()
                        && s.write_all(&data).await.is_err()
                    {
                        stdin = None;
                    }
                }
                ChannelMsg::Eof | ChannelMsg::Close => break,
                _ => {}
            }
        }
        drop(stdin);
    });
    let mut out = Vec::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = stdout.read(&mut buf).await.unwrap_or(0);
        if n == 0 {
            break;
        }
        if wr.data(&buf[..n]).await.is_err() {
            break;
        }
    }
    let _ = stderr.read_to_end(&mut out).await;
    if !out.is_empty() {
        let _ = wr.extended_data(1, &out[..]).await;
    }
    let status = child
        .wait()
        .await
        .ok()
        .and_then(|s| s.code())
        .unwrap_or(255);
    let _ = wr.exit_status(status as u32).await;
    let _ = wr.eof().await;
    let _ = wr.close().await;
    input.abort();
}

impl TestServer {
    fn start(allowed_keys: Vec<PublicKey>) -> TestServer {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let host = random_key();
        let host_key = host.public_key().clone();
        let config = Arc::new(server::Config {
            keys: vec![host],
            auth_rejection_time: Duration::from_millis(10),
            auth_rejection_time_initial: Some(Duration::ZERO),
            ..server::Config::default()
        });
        let auth_attempts = Arc::new(AtomicUsize::new(0));
        let forwards = Arc::new(Mutex::new(Vec::new()));
        let ptys = Arc::new(Mutex::new(Vec::new()));
        let handler = Handler {
            ptys: ptys.clone(),
            allowed_keys: Arc::new(allowed_keys),
            auth_attempts: auth_attempts.clone(),
            channels: Arc::default(),
            forwards: forwards.clone(),
        };
        let listener = rt
            .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        rt.spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let config = config.clone();
                let h = handler.clone();
                tokio::spawn(async move {
                    if let Ok(s) = server::run_stream(config, socket, h).await {
                        let _ = s.await;
                    }
                });
            }
        });
        TestServer {
            port,
            host_key,
            auth_attempts,
            forwards,
            ptys,
            _rt: rt,
        }
    }

    fn spec(&self, identity_files: Vec<PathBuf>) -> HostSpec {
        HostSpec {
            target: Target::parse(&format!("127.0.0.1:{}", self.port)).unwrap(),
            hostname: "127.0.0.1".into(),
            port: self.port,
            user: "tester".into(),
            identity_files,
            jumps: Vec::new(),
            proxy: None,
            route_error: None,
            agent_dir: None,
        }
    }
}

/// 答えを決めておく [`Prompter`]。尋ねられたことを記録する。
#[derive(Default)]
struct Answers {
    accept_host: bool,
    passwords: Mutex<Vec<String>>,
    passphrase: Option<String>,
    /// keyboard-interactive の答え
    interactive: Option<Vec<String>>,
    /// パスワードを「保存する」と答える
    save: bool,
    /// パスワードの問い合わせに添えられた説明
    notes: Mutex<Vec<String>>,
    log: Mutex<Vec<String>>,
}

impl Prompter for Answers {
    fn confirm_host_key(&self, q: &HostKeyQuestion) -> bool {
        let kind = match q.check {
            HostKeyCheck::Unknown => "unknown",
            HostKeyCheck::Changed { .. } => "changed",
        };
        self.log.lock().unwrap().push(format!("host:{kind}"));
        assert!(q.fingerprint.starts_with("SHA256:"));
        self.accept_host
    }

    fn password(&self, _: &str) -> Option<String> {
        self.log.lock().unwrap().push("password".into());
        let mut p = self.passwords.lock().unwrap();
        (!p.is_empty()).then(|| p.remove(0))
    }

    fn ask_password(&self, req: &PasswordRequest<'_>) -> Option<PasswordAnswer> {
        if let Some(n) = req.note {
            self.notes.lock().unwrap().push(n.to_owned());
        }
        assert!(req.can_save || !self.save);
        self.password(req.label).map(|password| PasswordAnswer {
            password,
            save: self.save,
        })
    }

    fn ask_passphrase(&self, req: &PassphraseRequest<'_>) -> Option<PasswordAnswer> {
        if let Some(n) = req.note {
            self.notes.lock().unwrap().push(n.to_owned());
        }
        assert!(req.can_save || !self.save);
        self.passphrase(req.key_file)
            .map(|password| PasswordAnswer {
                password,
                save: self.save,
            })
    }

    fn passphrase(&self, _: &Path) -> Option<String> {
        self.log.lock().unwrap().push("passphrase".into());
        self.passphrase.clone()
    }

    fn keyboard_interactive(
        &self,
        _: &str,
        _: &str,
        _: &str,
        prompts: &[(String, bool)],
    ) -> Option<Vec<String>> {
        self.log.lock().unwrap().push("keyboard-interactive".into());
        let a = self.interactive.clone()?;
        assert_eq!(a.len(), prompts.len());
        Some(a)
    }
}

impl Answers {
    fn log(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
}

fn connector(dir: &Path) -> SshConnector {
    SshConnector {
        known_hosts: dir.join("known_hosts"),
        extra_known_hosts: vec![dir.join("user_known_hosts")],
        keepalive: Duration::from_secs(15),
        passwords: None,
    }
}

fn record_host_key(file: &Path, server: &TestServer, key: &PublicKey) {
    let line = key.to_openssh().unwrap();
    fs::write(file, format!("[127.0.0.1]:{} {line}\n", server.port)).unwrap();
}

#[test]
fn asks_for_unknown_host_keys_and_remembers_them() {
    let server = TestServer::start(vec![]);
    let dir = tempfile::tempdir().unwrap();
    let c = connector(dir.path());

    // 承認しなければ認証の情報を送らずに切断する
    let p = Answers {
        passwords: Mutex::new(vec![PASSWORD.into()]),
        ..Answers::default()
    };
    let e = c
        .connect(&server.spec(vec![]), &p, &ConnectLog::new())
        .err()
        .unwrap();
    assert_eq!(e.kind(), std::io::ErrorKind::Interrupted, "{e}");
    assert_eq!(p.log(), ["host:unknown"]);
    assert_eq!(server.auth_attempts.load(Ordering::SeqCst), 0);
    assert!(!dir.path().join("known_hosts").exists());

    // 承認すれば記録し、次からは尋ねない
    let p = Answers {
        accept_host: true,
        passwords: Mutex::new(vec!["wrong".into(), PASSWORD.into()]),
        ..Answers::default()
    };
    let log = ConnectLog::new();
    let t = c.connect(&server.spec(vec![]), &p, &log).unwrap();
    assert_eq!(p.log(), ["host:unknown", "password", "password"]);
    assert!(
        log.contains("ホスト鍵の記録がありません"),
        "{:#?}",
        log.lines()
    );
    assert!(log.contains("ホスト鍵を承認しました"));
    assert!(log.contains("パスワード認証（1 回目）: 受け付けられませんでした"));
    assert!(!log.contains("wrong"));
    let out = yy_remote::run(t.as_ref(), b"echo hello", b"").unwrap();
    assert_eq!(out.stdout, b"hello\n");
    let p = Answers {
        passwords: Mutex::new(vec![PASSWORD.into()]),
        ..Answers::default()
    };
    c.connect(&server.spec(vec![]), &p, &ConnectLog::new())
        .unwrap();
    assert!(!p.log().iter().any(|l| l.starts_with("host:")));
}

#[test]
fn refuses_changed_host_keys_before_authenticating() {
    let server = TestServer::start(vec![]);
    let dir = tempfile::tempdir().unwrap();
    // 利用者の known_hosts（読むだけ）に別の鍵が記録されている
    let other = random_key();
    record_host_key(
        &dir.path().join("user_known_hosts"),
        &server,
        other.public_key(),
    );
    let p = Answers {
        accept_host: true,
        passwords: Mutex::new(vec![PASSWORD.into()]),
        ..Answers::default()
    };
    let log = ConnectLog::new();
    let e = connector(dir.path())
        .connect(&server.spec(vec![]), &p, &log)
        .err()
        .unwrap();
    assert!(log.contains("ホスト鍵が記録（"), "{:#?}", log.lines());
    assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied);
    assert!(e.to_string().contains("ホスト鍵が記録"), "{e}");
    assert_eq!(p.log(), ["host:changed"]);
    assert_eq!(server.auth_attempts.load(Ordering::SeqCst), 0);
}

#[test]
fn authenticates_with_an_encrypted_key() {
    let key = random_key();
    let server = TestServer::start(vec![key.public_key().clone()]);
    let dir = tempfile::tempdir().unwrap();
    record_host_key(&dir.path().join("known_hosts"), &server, &server.host_key);
    let encrypted = key.encrypt(&mut rand::rng(), PASSPHRASE).unwrap();
    let key_file = dir.path().join("id_ed25519");
    fs::write(
        &key_file,
        encrypted
            .to_openssh(russh::keys::ssh_key::LineEnding::LF)
            .unwrap()
            .as_bytes(),
    )
    .unwrap();
    let p = Answers {
        passphrase: Some(PASSPHRASE.into()),
        ..Answers::default()
    };
    let spec = server.spec(vec![dir.path().join("missing"), key_file]);
    let t = connector(dir.path())
        .connect(&spec, &p, &ConnectLog::new())
        .unwrap();
    assert_eq!(p.log(), ["passphrase"]);
    // 標準入力・標準エラー出力・終了コード
    let out = yy_remote::run(t.as_ref(), b"cat; echo oops >&2; exit 3", b"from stdin").unwrap();
    assert_eq!(out.stdout, b"from stdin");
    assert_eq!(out.stderr, b"oops\n");
    assert_eq!(out.status, Some(3));
    assert!(!t.is_closed());
}

#[test]
fn edits_a_file_through_the_agent_over_ssh() {
    let server = TestServer::start(vec![]);
    let dir = tempfile::tempdir().unwrap();
    record_host_key(&dir.path().join("known_hosts"), &server, &server.host_key);
    let agents = dir.path().join("agents");
    fs::create_dir(&agents).unwrap();
    fs::copy(
        env!("CARGO_BIN_EXE_yy-agent"),
        agents.join(format!("yy-agent-{}-linux", std::env::consts::ARCH)),
    )
    .unwrap();
    let mut spec = server.spec(vec![]);
    spec.agent_dir = Some(dir.path().join("remote").to_string_lossy().into_owned());
    let p = Answers {
        passwords: Mutex::new(vec![PASSWORD.into()]),
        ..Answers::default()
    };
    let c = connector(dir.path());
    let log = ConnectLog::new();
    let session =
        Arc::new(Remote::connect(&c, &spec, &p, &AgentFiles::new(&agents), &log).unwrap());
    // 接続の各段階が記録される
    for step in [
        "経路: 直接接続",
        "TCP で接続しました",
        "ホスト鍵は known_hosts の記録と一致しました",
        "パスワード認証で認証しました",
        "接続先の環境: Linux",
        "エージェントを配置します",
        "エージェントが起動しました",
    ] {
        assert!(log.contains(step), "{step}: {:#?}", log.lines());
    }
    assert!(!log.contains(PASSWORD));

    let file = dir.path().join("remote.txt");
    let original = "日本語のテキスト\n".repeat(200_000);
    fs::write(&file, &original).unwrap();
    let path = file.as_os_str().as_bytes();
    let mut got = Vec::new();
    let info = session.download(path, &mut got, &mut |_, _| true).unwrap();
    assert_eq!(got, original.as_bytes());

    let edited = original.replace("テキスト", "文書");
    let r = session
        .upload(&mut edited.as_bytes(), path, Some(info.id), &mut |_| true)
        .unwrap();
    assert!(matches!(r, UploadOutcome::Saved(_)));
    assert_eq!(fs::read_to_string(&file).unwrap(), edited);
    assert!(!session.is_closed());
}

#[test]
fn connects_through_jump_hosts() {
    let jump = TestServer::start(vec![]);
    let target = TestServer::start(vec![]);
    let dir = tempfile::tempdir().unwrap();
    let c = connector(dir.path());
    // 踏み台は名前を変えて、ホスト鍵を別々に確かめることを見る
    let mut hop = jump.spec(vec![]);
    hop.hostname = "localhost".into();
    let mut spec = target.spec(vec![]);
    spec.jumps = vec![hop];
    let p = Answers {
        accept_host: true,
        passwords: Mutex::new(vec![PASSWORD.into(), PASSWORD.into()]),
        ..Answers::default()
    };
    let log = ConnectLog::new();
    let t = c.connect(&spec, &p, &log).unwrap();
    assert_eq!(
        p.log(),
        ["host:unknown", "password", "host:unknown", "password"]
    );
    for step in [
        "踏み台 1: tester@localhost:",
        "（踏み台経由）",
        "踏み台の上で 127.0.0.1:",
        "SSH の接続と認証が済みました",
    ] {
        assert!(log.contains(step), "{step}: {:#?}", log.lines());
    }
    assert_eq!(
        *jump.forwards.lock().unwrap(),
        [format!("127.0.0.1:{}", target.port)]
    );
    let out = yy_remote::run(t.as_ref(), b"echo via jump", b"").unwrap();
    assert_eq!(out.stdout, b"via jump\n");
    assert!(!t.is_closed());
    let known = fs::read_to_string(dir.path().join("known_hosts")).unwrap();
    assert!(
        known.contains(&format!("[localhost]:{}", jump.port)),
        "{known}"
    );
    assert!(
        known.contains(&format!("[127.0.0.1]:{}", target.port)),
        "{known}"
    );

    // 踏み台から先に接続できない
    let mut spec = target.spec(vec![]);
    spec.port = closed_port();
    let mut hop = jump.spec(vec![]);
    hop.hostname = "localhost".into();
    spec.jumps = vec![hop];
    let p = Answers {
        passwords: Mutex::new(vec![PASSWORD.into()]),
        ..Answers::default()
    };
    let e = c.connect(&spec, &p, &ConnectLog::new()).err().unwrap();
    assert!(e.to_string().contains("踏み台から"), "{e}");

    // 踏み台の認証を中止した
    let mut spec = target.spec(vec![]);
    spec.jumps = vec![jump.spec(vec![])];
    record_host_key(&dir.path().join("known_hosts"), &jump, &jump.host_key);
    let e = c
        .connect(&spec, &Answers::default(), &ConnectLog::new())
        .err()
        .unwrap();
    assert_eq!(e.kind(), std::io::ErrorKind::Interrupted, "{e}");
    assert!(e.to_string().starts_with("踏み台 tester@127.0.0.1"), "{e}");

    // 設定の誤りは接続せずに知らせる
    let mut spec = target.spec(vec![]);
    spec.route_error = Some("ProxyCommand（x）には対応していません".into());
    let e = c
        .connect(&spec, &Answers::default(), &ConnectLog::new())
        .err()
        .unwrap();
    assert!(e.to_string().contains("ProxyCommand"), "{e}");
}

/// 使われていないポート。
fn closed_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// テスト用のプロキシ（HTTP CONNECT と、認証なしの SOCKS5）。中継を頼まれた接続先を記録する。
struct TestProxy {
    port: u16,
    requests: Arc<Mutex<Vec<String>>>,
    _rt: tokio::runtime::Runtime,
}

impl TestProxy {
    /// `credentials` は HTTP の `Proxy-Authorization` に期待する値。
    fn start(credentials: Option<&'static str>) -> TestProxy {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let listener = rt
            .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let log = requests.clone();
        rt.spawn(async move {
            while let Ok((mut s, _)) = listener.accept().await {
                let log = log.clone();
                tokio::spawn(async move {
                    let Some((host, port)) = accept_proxy(&mut s, credentials).await else {
                        return;
                    };
                    log.lock().unwrap().push(format!("{host}:{port}"));
                    if let Ok(mut out) = tokio::net::TcpStream::connect((host, port)).await {
                        let _ = tokio::io::copy_bidirectional(&mut s, &mut out).await;
                    }
                });
            }
        });
        TestProxy {
            port,
            requests,
            _rt: rt,
        }
    }
}

async fn accept_proxy(
    s: &mut tokio::net::TcpStream,
    credentials: Option<&str>,
) -> Option<(String, u16)> {
    let first = s.read_u8().await.ok()?;
    if first == 5 {
        let n = s.read_u8().await.ok()?;
        let mut methods = vec![0u8; usize::from(n)];
        s.read_exact(&mut methods).await.ok()?;
        s.write_all(&[5, 0]).await.ok()?;
        let mut head = [0u8; 4];
        s.read_exact(&mut head).await.ok()?;
        let host = match head[3] {
            1 => {
                let mut ip = [0u8; 4];
                s.read_exact(&mut ip).await.ok()?;
                std::net::Ipv4Addr::from(ip).to_string()
            }
            3 => {
                let len = s.read_u8().await.ok()?;
                let mut host = vec![0u8; usize::from(len)];
                s.read_exact(&mut host).await.ok()?;
                String::from_utf8(host).ok()?
            }
            _ => return None,
        };
        let port = s.read_u16().await.ok()?;
        s.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).await.ok()?;
        return Some((host, port));
    }
    let mut head = vec![first];
    while !head.ends_with(b"\r\n\r\n") {
        head.push(s.read_u8().await.ok()?);
    }
    let head = String::from_utf8(head).ok()?;
    let target = head.strip_prefix("CONNECT ")?.split(' ').next()?.to_owned();
    if let Some(c) = credentials
        && !head.contains(&format!("Proxy-Authorization: Basic {c}\r\n"))
    {
        s.write_all(b"HTTP/1.1 407 Proxy Authentication Required\r\n\r\n")
            .await
            .ok()?;
        return None;
    }
    s.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
        .await
        .ok()?;
    let (host, port) = target.rsplit_once(':')?;
    Some((host.to_owned(), port.parse().ok()?))
}

#[test]
fn connects_through_proxies() {
    let server = TestServer::start(vec![]);
    let dir = tempfile::tempdir().unwrap();
    record_host_key(&dir.path().join("known_hosts"), &server, &server.host_key);
    let c = connector(dir.path());
    let password = || Answers {
        passwords: Mutex::new(vec![PASSWORD.into()]),
        ..Answers::default()
    };

    // SOCKS5
    let socks = TestProxy::start(None);
    let mut spec = server.spec(vec![]);
    spec.proxy = Proxy::parse(&format!("socks5://127.0.0.1:{}", socks.port)).unwrap();
    let t = c.connect(&spec, &password(), &ConnectLog::new()).unwrap();
    let out = yy_remote::run(t.as_ref(), b"echo socks", b"").unwrap();
    assert_eq!(out.stdout, b"socks\n");
    assert_eq!(
        *socks.requests.lock().unwrap(),
        [format!("127.0.0.1:{}", server.port)]
    );

    // HTTP CONNECT（ユーザー名だけを書いたのでパスワードを尋ねる）
    let http = TestProxy::start(Some("YWxpY2U6c2VjcmV0"));
    let mut spec = server.spec(vec![]);
    spec.proxy = Proxy::parse(&format!("http://alice@127.0.0.1:{}", http.port)).unwrap();
    let p = Answers {
        passwords: Mutex::new(vec!["secret".into(), PASSWORD.into()]),
        ..Answers::default()
    };
    let t = c.connect(&spec, &p, &ConnectLog::new()).unwrap();
    assert_eq!(p.log(), ["password", "password"]);
    let out = yy_remote::run(t.as_ref(), b"echo http", b"").unwrap();
    assert_eq!(out.stdout, b"http\n");

    // プロキシのパスワードを間違えたら尋ね直す
    let p = Answers {
        passwords: Mutex::new(vec!["wrong".into(), "secret".into(), PASSWORD.into()]),
        ..Answers::default()
    };
    c.connect(&spec, &p, &ConnectLog::new()).unwrap();
    assert_eq!(p.log(), ["password", "password", "password"]);
    // 何度も間違えたらあきらめる
    let p = Answers {
        passwords: Mutex::new(vec!["wrong".into(); 4]),
        ..Answers::default()
    };
    let e = c.connect(&spec, &p, &ConnectLog::new()).err().unwrap();
    assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied, "{e}");
    assert!(e.to_string().contains("プロキシの認証"), "{e}");
    assert_eq!(p.log().len(), 4);

    // ユーザー名を書いていなければ、プロキシに求められたときにユーザー名とパスワードを尋ねる
    let mut spec = server.spec(vec![]);
    spec.proxy = Proxy::parse(&format!("http://127.0.0.1:{}", http.port)).unwrap();
    let p = Answers {
        passwords: Mutex::new(vec!["secret".into(), PASSWORD.into()]),
        interactive: Some(vec!["alice".into()]),
        ..Answers::default()
    };
    c.connect(&spec, &p, &ConnectLog::new()).unwrap();
    assert_eq!(p.log(), ["keyboard-interactive", "password", "password"]);
    // 答えなければ中止
    let e = c
        .connect(&spec, &Answers::default(), &ConnectLog::new())
        .err()
        .unwrap();
    assert_eq!(e.kind(), std::io::ErrorKind::Interrupted, "{e}");

    // プロキシは最初の踏み台への接続にだけ使う
    let target = TestServer::start(vec![]);
    record_host_key(
        &dir.path().join("user_known_hosts"),
        &target,
        &target.host_key,
    );
    let mut hop = server.spec(vec![]);
    hop.proxy = Proxy::parse(&format!("socks5://127.0.0.1:{}", socks.port)).unwrap();
    let mut spec = target.spec(vec![]);
    spec.jumps = vec![hop];
    let p = Answers {
        passwords: Mutex::new(vec![PASSWORD.into(), PASSWORD.into()]),
        ..Answers::default()
    };
    let t = c.connect(&spec, &p, &ConnectLog::new()).unwrap();
    assert_eq!(socks.requests.lock().unwrap().len(), 2);
    assert_eq!(
        *server.forwards.lock().unwrap(),
        [format!("127.0.0.1:{}", target.port)]
    );
    assert_eq!(
        yy_remote::run(t.as_ref(), b"true", b"").unwrap().status,
        Some(0)
    );
}

#[test]
fn logs_why_a_connection_failed() {
    let dir = tempfile::tempdir().unwrap();
    let c = connector(dir.path());
    let mut spec = TestServer::start(vec![]).spec(vec![dir.path().join("id_missing")]);
    spec.port = closed_port();
    let log = ConnectLog::new();
    let e = c.connect(&spec, &Answers::default(), &log).err().unwrap();
    assert!(e.to_string().contains("に接続できませんでした"), "{e}");
    assert!(log.contains("id_missing（なし）"), "{:#?}", log.lines());
    assert!(log.contains("に接続します（直接）"));

    // 読めない秘密鍵と、受け付けられない認証
    let server = TestServer::start(vec![]);
    record_host_key(&dir.path().join("known_hosts"), &server, &server.host_key);
    let bad = dir.path().join("id_bad");
    fs::write(&bad, "not a key").unwrap();
    let log = ConnectLog::new();
    let p = Answers {
        passwords: Mutex::new(vec!["x".into(); 3]),
        ..Answers::default()
    };
    let e = c.connect(&server.spec(vec![bad]), &p, &log).err().unwrap();
    assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied);
    assert!(e.to_string().contains("サーバーが受け付ける方法"), "{e}");
    assert!(
        log.contains("id_bad を読めないため使いません"),
        "{:#?}",
        log.lines()
    );
    assert!(log.contains("パスワード認証（3 回目）: 受け付けられませんでした"));
}

#[test]
fn remembers_passwords() {
    let server = TestServer::start(vec![]);
    let dir = tempfile::tempdir().unwrap();
    record_host_key(&dir.path().join("known_hosts"), &server, &server.host_key);
    let store = Arc::new(MemoryPasswords::default());
    let mut c = connector(dir.path());
    c.passwords = Some(store.clone());
    let key = format!("ssh/tester@127.0.0.1:{}", server.port);

    // 認証に成功したパスワードだけを保存する
    let p = Answers {
        passwords: Mutex::new(vec!["wrong".into(), PASSWORD.into()]),
        save: true,
        ..Answers::default()
    };
    c.connect(&server.spec(vec![]), &p, &ConnectLog::new())
        .unwrap();
    assert_eq!(p.log(), ["password", "password"]);
    assert_eq!(store.load(&key).unwrap().password, PASSWORD);

    // 次からは尋ねない
    let p = Answers::default();
    let log = ConnectLog::new();
    c.connect(&server.spec(vec![]), &p, &log).unwrap();
    assert!(p.log().is_empty(), "{:?}", p.log());
    assert!(log.contains("パスワード認証（保存したパスワード）で認証しました"));
    assert!(!log.contains(PASSWORD));

    // 受け付けられなかった保存は消して尋ねる
    store
        .save(
            &key,
            &SavedPassword {
                user: "tester".into(),
                password: "old".into(),
            },
        )
        .unwrap();
    let p = Answers {
        passwords: Mutex::new(vec![PASSWORD.into()]),
        ..Answers::default()
    };
    let log = ConnectLog::new();
    c.connect(&server.spec(vec![]), &p, &log).unwrap();
    assert_eq!(p.log(), ["password"]);
    assert!(p.notes.lock().unwrap()[0].contains("保存したパスワードは受け付けられませんでした"));
    assert!(log.contains("保存を削除しました"));
    assert!(store.keys().is_empty());

    // プロキシのユーザー名とパスワードも保存できる
    let http = TestProxy::start(Some("YWxpY2U6c2VjcmV0"));
    let mut spec = server.spec(vec![]);
    spec.proxy = Proxy::parse(&format!("http://127.0.0.1:{}", http.port)).unwrap();
    let p = Answers {
        passwords: Mutex::new(vec!["secret".into(), PASSWORD.into()]),
        interactive: Some(vec!["alice".into()]),
        save: true,
        ..Answers::default()
    };
    c.connect(&spec, &p, &ConnectLog::new()).unwrap();
    let proxy_key = format!("proxy/http://127.0.0.1:{}", http.port);
    assert_eq!(store.load(&proxy_key).unwrap().user, "alice");
    let p = Answers::default();
    c.connect(&spec, &p, &ConnectLog::new()).unwrap();
    assert!(p.log().is_empty(), "{:?}", p.log());
}

#[test]
fn remembers_passphrases_when_asked() {
    let key = random_key();
    let server = TestServer::start(vec![key.public_key().clone()]);
    let dir = tempfile::tempdir().unwrap();
    record_host_key(&dir.path().join("known_hosts"), &server, &server.host_key);
    let key_file = dir.path().join("id_ed25519");
    let encrypted = key.encrypt(&mut rand::rng(), PASSPHRASE).unwrap();
    fs::write(
        &key_file,
        encrypted
            .to_openssh(russh::keys::ssh_key::LineEnding::LF)
            .unwrap()
            .as_bytes(),
    )
    .unwrap();
    let store = Arc::new(MemoryPasswords::default());
    let mut c = connector(dir.path());
    c.passwords = Some(store.clone());
    let spec = server.spec(vec![key_file.clone()]);
    let saved_key = yy_remote::passphrase_key(&key_file);

    // 選ばなければ保存しない
    let p = Answers {
        passphrase: Some(PASSPHRASE.into()),
        ..Answers::default()
    };
    c.connect(&spec, &p, &ConnectLog::new()).unwrap();
    assert!(store.keys().is_empty());

    // 選べば、鍵を開けたパスフレーズを保存し、次からは尋ねない
    let p = Answers {
        passphrase: Some(PASSPHRASE.into()),
        save: true,
        ..Answers::default()
    };
    c.connect(&spec, &p, &ConnectLog::new()).unwrap();
    assert_eq!(store.load(&saved_key).unwrap().password, PASSPHRASE);
    let p = Answers::default();
    let log = ConnectLog::new();
    c.connect(&spec, &p, &log).unwrap();
    assert!(p.log().is_empty(), "{:?}", p.log());
    assert!(
        log.contains("保存したパスフレーズで開きました"),
        "{:#?}",
        log.lines()
    );
    assert!(!log.contains(PASSPHRASE));

    // 開けなければ保存を消して尋ねる
    store
        .save(
            &saved_key,
            &SavedPassword {
                user: String::new(),
                password: "old".into(),
            },
        )
        .unwrap();
    let p = Answers {
        passphrase: Some(PASSPHRASE.into()),
        ..Answers::default()
    };
    c.connect(&spec, &p, &ConnectLog::new()).unwrap();
    assert_eq!(p.log(), ["passphrase"]);
    assert!(p.notes.lock().unwrap()[0].contains("保存したパスフレーズでは開けませんでした"));
    assert!(store.keys().is_empty());
}

#[test]
fn opens_interactive_shells() {
    use std::io::{Read, Write};
    let server = TestServer::start(vec![]);
    let dir = tempfile::tempdir().unwrap();
    record_host_key(&dir.path().join("known_hosts"), &server, &server.host_key);
    let p = Answers {
        passwords: Mutex::new(vec![PASSWORD.into()]),
        ..Answers::default()
    };
    let t = connector(dir.path())
        .connect(&server.spec(vec![]), &p, &ConnectLog::new())
        .unwrap();

    // ログインシェル: 入力したコマンドを実行し、終了コードを返す
    let mut sh = t.shell("xterm-256color", (80, 24), None).unwrap();
    (sh.resize)(100, 30);
    sh.input
        .write_all(b"echo hello; echo oops >&2; exit 3\n")
        .unwrap();
    sh.input.flush().unwrap();
    let mut out = Vec::new();
    sh.output.read_to_end(&mut out).unwrap();
    let out = String::from_utf8(out).unwrap();
    assert!(out.contains("hello") && out.contains("oops"), "{out:?}");
    assert_eq!((sh.finish)().unwrap().status, Some(3));
    let ptys = server.ptys.lock().unwrap().clone();
    assert_eq!(ptys[0], "pty xterm-256color 80x24");
    assert!(ptys.contains(&"resize 100x30".to_owned()), "{ptys:?}");

    // コマンドを端末つきで実行する（接続先のフォルダで始めるのに使う）
    let mut sh = t
        .shell("xterm-256color", (80, 24), Some(b"cd /tmp && pwd"))
        .unwrap();
    drop(sh.input);
    let mut out = String::new();
    sh.output.read_to_string(&mut out).unwrap();
    assert_eq!(out.trim(), "/tmp");
}

#[test]
fn transfers_files_over_sftp_with_the_journal() {
    use yy_remote::xfer;
    if yy_remote::local::sftp_server().is_none() {
        eprintln!("sftp-server がないため飛ばします");
        return;
    }
    let server = TestServer::start(vec![]);
    let dir = tempfile::tempdir().unwrap();
    record_host_key(&dir.path().join("known_hosts"), &server, &server.host_key);
    let store = Arc::new(MemoryPasswords::default());
    store
        .save(
            &format!("ssh/tester@127.0.0.1:{}", server.port),
            &SavedPassword {
                user: "tester".into(),
                password: PASSWORD.into(),
            },
        )
        .unwrap();
    let mut c = connector(dir.path());
    c.passwords = Some(store);
    let spec = server.spec(vec![]);

    let src = dir.path().join("big.bin");
    let content: Vec<u8> = (0..5_000_000u32).map(|i| (i * 7 % 251) as u8).collect();
    fs::write(&src, &content).unwrap();
    let dst = dir.path().join("up/big.bin");
    let remote = yy_remote::RemoteUri {
        user: None,
        host: "127.0.0.1".into(),
        port: Some(server.port),
        path: dst.as_os_str().as_bytes().to_vec(),
    };
    let log = yy_remote::log::TransferLog::new(None, || "T".into());
    let connect = |l: &yy_remote::log::TransferLog, id: u64| {
        let cl = ConnectLog::new();
        let r = c.connect(&spec, &Answers::default(), &cl);
        l.connect_log(Some(id), &cl);
        r
    };
    let journal = xfer::Journal::new(dir.path().join("journal"));
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let mut progress = |_: &xfer::Job| {};
    let mut cx = xfer::Context {
        connect: &connect,
        log: &log,
        journal: Some(&journal),
        cancel: &cancel,
        progress: &mut progress,
        retry: xfer::Retry::default(),
        scp_chunk: 1 << 20,
        transport: None,
    };
    let mut job = xfer::upload_job(1, xfer::Protocol::Sftp, &src, remote.clone(), false).unwrap();
    xfer::run(&mut job, &mut cx);
    assert_eq!(job.state, xfer::State::Done, "{}", job.message);
    assert_eq!(fs::read(&dst).unwrap(), content);

    // 同じ接続で受け取る
    let back = dir.path().join("down/big.bin");
    let meta = fs::metadata(&dst).unwrap();
    let mut job = xfer::download_job(
        2,
        xfer::Protocol::Sftp,
        remote,
        meta.len(),
        xfer::mtime_of(&meta),
        &back,
        false,
    );
    xfer::run(&mut job, &mut cx);
    assert_eq!(job.state, xfer::State::Done, "{}", job.message);
    assert_eq!(fs::read(&back).unwrap(), content);
    assert!(journal.load().is_empty());
}
