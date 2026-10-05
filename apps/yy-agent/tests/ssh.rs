//! 組み込みの SSH クライアント（yy-ssh）の結合テスト。
//!
//! russh のサーバーを同じプロセスで立て（exec 要求は手元の `sh -c` で実行）、OpenSSH を使わずに
//! ホスト鍵の確認・認証・コマンド実行・エージェントの配置と保存までを確かめる。
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
use russh::{Channel, ChannelId, ChannelMsg, MethodKind, MethodSet};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use yy_remote::uri::Target;
use yy_remote::{
    AgentFiles, Connector, HostKeyCheck, HostKeyQuestion, HostSpec, Prompter, Session as Remote,
    UploadOutcome,
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
    _rt: tokio::runtime::Runtime,
}

#[derive(Clone)]
struct Handler {
    allowed_keys: Arc<Vec<PublicKey>>,
    auth_attempts: Arc<AtomicUsize>,
    channels: Arc<Mutex<HashMap<ChannelId, Channel<Msg>>>>,
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
        let handler = Handler {
            allowed_keys: Arc::new(allowed_keys),
            auth_attempts: auth_attempts.clone(),
            channels: Arc::default(),
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
            proxy_jump: None,
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

    fn passphrase(&self, _: &Path) -> Option<String> {
        self.log.lock().unwrap().push("passphrase".into());
        self.passphrase.clone()
    }

    fn keyboard_interactive(
        &self,
        _: &str,
        _: &str,
        _: &str,
        _: &[(String, bool)],
    ) -> Option<Vec<String>> {
        self.log.lock().unwrap().push("keyboard-interactive".into());
        None
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
    let e = c.connect(&server.spec(vec![]), &p).err().unwrap();
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
    let t = c.connect(&server.spec(vec![]), &p).unwrap();
    assert_eq!(p.log(), ["host:unknown", "password", "password"]);
    let out = yy_remote::run(t.as_ref(), b"echo hello", b"").unwrap();
    assert_eq!(out.stdout, b"hello\n");
    let p = Answers {
        passwords: Mutex::new(vec![PASSWORD.into()]),
        ..Answers::default()
    };
    c.connect(&server.spec(vec![]), &p).unwrap();
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
    let e = connector(dir.path())
        .connect(&server.spec(vec![]), &p)
        .err()
        .unwrap();
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
    let t = connector(dir.path()).connect(&spec, &p).unwrap();
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
    let session = Arc::new(Remote::connect(&c, &spec, &p, &AgentFiles::new(&agents)).unwrap());

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
