//! yyeditor に組み込む SSH クライアント（11 章 4）。
//!
//! SSH プロトコルは Pure Rust の `russh`（暗号処理は `ring`）で話し、`ssh.exe` など OpenSSH の
//! プログラムは使わない（C1）。非同期実行（tokio）はこのクレートの中に閉じ込め、外には
//! [`yy_remote::Connector`] と [`yy_remote::Transport`] の同期的な trait だけを見せる。
//!
//! ホスト鍵は、鍵交換（サーバーの署名の検証）が済んだ時点で受け取っておき、認証の情報を
//! 送る前に `known_hosts` と照合する。初めてのホストは利用者に確かめてから記録し、記録と
//! 違う鍵なら認証せずに切断する。

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use russh::client::{self, AuthResult, Handle, KeyboardInteractiveAuthResponse};
use russh::keys::{PrivateKey, PrivateKeyWithHashAlg, PublicKey, PublicKeyOrCertificate};
use russh::{ChannelMsg, MethodKind};
use tokio::io::AsyncWriteExt;
use tokio::runtime::Runtime;
use tokio::sync::mpsc;
use yy_remote::known_hosts::{self, HostKeyStatus};
use yy_remote::{
    Connector, ConnectorFactory, ConnectorOptions, Exit, HostKeyCheck, HostKeyQuestion, HostSpec,
    Process, Prompter, STDERR_LIMIT, Transport,
};

/// 接続を待つ時間の上限
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// パスワード・パスフレーズを尋ね直す回数
const RETRIES: usize = 3;

/// SSH で接続する [`Connector`]。
pub struct SshConnector {
    /// yyeditor 自身のホスト鍵の記録（承認した鍵を書き込む）
    pub known_hosts: PathBuf,
    /// 読むだけのホスト鍵の記録（`~/.ssh/known_hosts` など）
    pub extra_known_hosts: Vec<PathBuf>,
    /// 死活確認の間隔（応答が 3 回続けてなければ切断とみなす）
    pub keepalive: Duration,
}

/// 非同期実行の環境。接続するまで作らない（起動を遅くしない）。
fn runtime() -> io::Result<&'static Runtime> {
    static RT: OnceLock<Runtime> = OnceLock::new();
    if let Some(rt) = RT.get() {
        return Ok(rt);
    }
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .thread_name("yy-ssh")
        .enable_all()
        .build()?;
    Ok(RT.get_or_init(|| rt))
}

fn ssh_error(e: russh::Error) -> io::Error {
    match e {
        russh::Error::IO(e) => e,
        e => io::Error::other(e.to_string()),
    }
}

/// russh から呼ばれる処理。ホスト鍵は受け取っておくだけで、照合は認証の前に行う。
struct Client {
    server_key: Arc<Mutex<Option<PublicKey>>>,
}

impl client::Handler for Client {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let key = match key {
            PublicKeyOrCertificate::PublicKey { key, .. } => key.clone(),
            // 証明書は認証局を使わず、中の公開鍵で照合する
            PublicKeyOrCertificate::Certificate(c) => PublicKey::new(c.public_key().clone(), ""),
        };
        *self.server_key.lock().unwrap() = Some(key);
        Ok(true)
    }
}

impl SshConnector {
    pub fn new(opts: &ConnectorOptions) -> SshConnector {
        SshConnector {
            known_hosts: opts.known_hosts.clone(),
            extra_known_hosts: opts.extra_known_hosts.clone(),
            keepalive: opts.keepalive,
        }
    }

    /// UI に渡す、[`SshConnector`] を作る関数。
    pub fn factory() -> ConnectorFactory {
        Arc::new(|opts: &ConnectorOptions| -> Arc<dyn Connector> {
            Arc::new(SshConnector::new(opts))
        })
    }

    fn known_hosts_files(&self) -> Vec<PathBuf> {
        let mut files = vec![self.known_hosts.clone()];
        files.extend(self.extra_known_hosts.iter().cloned());
        files
    }

    /// ホスト鍵を照合し、初めてのホストなら利用者に確かめて記録する。
    fn verify_host_key(
        &self,
        spec: &HostSpec,
        key: &PublicKey,
        prompter: &dyn Prompter,
    ) -> io::Result<()> {
        let openssh = key
            .to_openssh()
            .map_err(|e| io::Error::other(e.to_string()))?;
        let mut words = openssh.split_whitespace();
        let (Some(algorithm), Some(b64)) = (words.next(), words.next()) else {
            return Err(io::Error::other("ホスト鍵を読めません"));
        };
        let name = spec.known_hosts_name();
        let status = known_hosts::check(&self.known_hosts_files(), &name, algorithm, b64);
        let check = match status {
            HostKeyStatus::Known => return Ok(()),
            HostKeyStatus::Unknown => HostKeyCheck::Unknown,
            HostKeyStatus::Changed { file, line } => HostKeyCheck::Changed { file, line },
        };
        let question = HostKeyQuestion {
            host: spec.hostname.clone(),
            port: spec.port,
            algorithm: algorithm.to_owned(),
            fingerprint: known_hosts::fingerprint(b64).unwrap_or_default(),
            check: check.clone(),
        };
        let accepted = prompter.confirm_host_key(&question);
        match check {
            HostKeyCheck::Changed { file, line } => Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "{} のホスト鍵が記録（{} の {line} 行目）と違うため接続しませんでした",
                    spec.hostname,
                    file.display()
                ),
            )),
            HostKeyCheck::Unknown if accepted => {
                known_hosts::learn(&self.known_hosts, &name, algorithm, b64)
            }
            HostKeyCheck::Unknown => Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "ホスト鍵を承認しなかったため接続しませんでした",
            )),
        }
    }
}

impl Connector for SshConnector {
    fn connect(&self, spec: &HostSpec, prompter: &dyn Prompter) -> io::Result<Arc<dyn Transport>> {
        if let Some(jump) = &spec.proxy_jump {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("踏み台（ProxyJump {jump}）経由の接続にはまだ対応していません"),
            ));
        }
        let rt = runtime()?;
        let config = Arc::new(client::Config {
            keepalive_interval: Some(self.keepalive),
            keepalive_max: 3,
            inactivity_timeout: None,
            nodelay: true,
            ..client::Config::default()
        });
        let server_key = Arc::new(Mutex::new(None));
        let handler = Client {
            server_key: server_key.clone(),
        };
        let addr = (spec.hostname.clone(), spec.port);
        let mut handle = rt.block_on(async {
            match tokio::time::timeout(CONNECT_TIMEOUT, client::connect(config, addr, handler))
                .await
            {
                Ok(r) => r.map_err(ssh_error),
                Err(_) => Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!(
                        "{}:{} に接続できませんでした（時間切れ）",
                        spec.hostname, spec.port
                    ),
                )),
            }
        })?;
        let key = server_key
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| io::Error::other("ホスト鍵を受け取れませんでした"))?;
        if let Err(e) = self.verify_host_key(spec, &key, prompter) {
            disconnect(rt, &handle);
            return Err(e);
        }
        if let Err(e) = authenticate(rt, &mut handle, spec, prompter) {
            disconnect(rt, &handle);
            return Err(e);
        }
        Ok(Arc::new(SshTransport {
            handle: Arc::new(handle),
        }))
    }
}

fn disconnect(rt: &Runtime, handle: &Handle<Client>) {
    let _ = rt.block_on(handle.disconnect(russh::Disconnect::ByApplication, "", "en"));
}

/// 公開鍵 → keyboard-interactive → パスワードの順に試す。
fn authenticate(
    rt: &Runtime,
    handle: &mut Handle<Client>,
    spec: &HostSpec,
    prompter: &dyn Prompter,
) -> io::Result<()> {
    let user = spec.user.clone();
    // サーバーが受け付ける方法（最初は分からないので、すべて試す）
    let mut methods: Option<Vec<MethodKind>> = None;
    let allowed =
        |m: &Option<Vec<MethodKind>>, k: MethodKind| m.as_ref().is_none_or(|v| v.contains(&k));
    let note = |r: &AuthResult, m: &mut Option<Vec<MethodKind>>| {
        if let AuthResult::Failure {
            remaining_methods, ..
        } = r
        {
            *m = Some(remaining_methods.iter().copied().collect());
        }
    };

    for path in spec.identity_files.iter().filter(|p| p.is_file()) {
        if !allowed(&methods, MethodKind::PublicKey) {
            break;
        }
        let Some(key) = load_key(path, prompter)? else {
            continue;
        };
        let hash = rt
            .block_on(handle.best_supported_rsa_hash())
            .map_err(ssh_error)?
            .flatten();
        let key = PrivateKeyWithHashAlg::new(Arc::new(key), hash);
        let r = rt
            .block_on(handle.authenticate_publickey(user.clone(), key))
            .map_err(ssh_error)?;
        if r.success() {
            return Ok(());
        }
        note(&r, &mut methods);
    }

    if allowed(&methods, MethodKind::KeyboardInteractive) {
        let user_host = spec.user_host();
        let mut r = rt
            .block_on(handle.authenticate_keyboard_interactive_start(user.clone(), None))
            .map_err(ssh_error)?;
        loop {
            match r {
                KeyboardInteractiveAuthResponse::Success => return Ok(()),
                KeyboardInteractiveAuthResponse::Failure {
                    remaining_methods, ..
                } => {
                    methods = Some(remaining_methods.iter().copied().collect());
                    break;
                }
                KeyboardInteractiveAuthResponse::InfoRequest {
                    name,
                    instructions,
                    prompts,
                } => {
                    let answers = if prompts.is_empty() {
                        Vec::new()
                    } else {
                        let q: Vec<(String, bool)> =
                            prompts.iter().map(|p| (p.prompt.clone(), p.echo)).collect();
                        prompter
                            .keyboard_interactive(&user_host, &name, &instructions, &q)
                            .ok_or_else(cancelled)?
                    };
                    r = rt
                        .block_on(handle.authenticate_keyboard_interactive_respond(answers))
                        .map_err(ssh_error)?;
                }
            }
        }
    }

    if allowed(&methods, MethodKind::Password) {
        for _ in 0..RETRIES {
            let password = prompter.password(&spec.user_host()).ok_or_else(cancelled)?;
            let r = rt
                .block_on(handle.authenticate_password(user.clone(), password))
                .map_err(ssh_error)?;
            if r.success() {
                return Ok(());
            }
            note(&r, &mut methods);
            if !allowed(&methods, MethodKind::Password) {
                break;
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!("{} に認証できませんでした", spec.user_host()),
    ))
}

fn cancelled() -> io::Error {
    io::Error::new(io::ErrorKind::Interrupted, "接続を中止しました")
}

/// 秘密鍵を読む。暗号化されていればパスフレーズを尋ねる（答えなければ `None`）。
fn load_key(path: &Path, prompter: &dyn Prompter) -> io::Result<Option<PrivateKey>> {
    match russh::keys::load_secret_key(path, None) {
        Ok(k) => return Ok(Some(k)),
        Err(russh::keys::Error::KeyIsEncrypted) => {}
        // 読めない鍵（対応していない形式など）は使わない
        Err(_) => return Ok(None),
    }
    for _ in 0..RETRIES {
        let Some(pass) = prompter.passphrase(path) else {
            return Ok(None);
        };
        if let Ok(k) = russh::keys::load_secret_key(path, Some(&pass)) {
            return Ok(Some(k));
        }
    }
    Ok(None)
}

/// 認証済みの SSH 接続。
struct SshTransport {
    handle: Arc<Handle<Client>>,
}

impl Drop for SshTransport {
    fn drop(&mut self) {
        if let Ok(rt) = runtime() {
            let h = self.handle.clone();
            rt.spawn(async move {
                let _ = h
                    .disconnect(russh::Disconnect::ByApplication, "", "en")
                    .await;
            });
        }
    }
}

impl Transport for SshTransport {
    fn exec(&self, command: &[u8]) -> io::Result<Process> {
        let rt = runtime()?;
        let channel = rt.block_on(async {
            let ch = self.handle.channel_open_session().await?;
            ch.exec(true, command.to_vec()).await?;
            Ok::<_, russh::Error>(ch)
        });
        let channel = channel.map_err(ssh_error)?;
        let (mut read_half, write_half) = channel.split();
        // 標準出力は数を限った受け渡し口に流す（読む側が遅ければ SSH のウィンドウで送信を止める）
        let (out_tx, out_rx) = mpsc::channel::<Vec<u8>>(64);
        let (exit_tx, exit_rx) = std::sync::mpsc::channel::<Exit>();
        rt.spawn(async move {
            let mut exit = Exit::default();
            let mut out = Some(out_tx);
            while let Some(msg) = read_half.wait().await {
                match msg {
                    ChannelMsg::Data { data } => {
                        if let Some(tx) = &out
                            && tx.send(data.to_vec()).await.is_err()
                        {
                            // 読む側がいなくなった
                            out = None;
                        }
                    }
                    ChannelMsg::ExtendedData { data, ext: 1 } => {
                        let room = STDERR_LIMIT.saturating_sub(exit.stderr.len());
                        exit.stderr.extend_from_slice(&data[..data.len().min(room)]);
                    }
                    ChannelMsg::ExitStatus { exit_status } => exit.status = Some(exit_status),
                    ChannelMsg::Eof => out = None,
                    // コマンドを起動できなかった
                    ChannelMsg::Failure => {
                        exit.stderr
                            .extend_from_slice("コマンドを実行できませんでした".as_bytes());
                        break;
                    }
                    ChannelMsg::Close => break,
                    _ => {}
                }
            }
            drop(out);
            let _ = exit_tx.send(exit);
        });
        let writer = ChannelWriter {
            inner: Some(Box::pin(write_half.make_writer())),
            half: Some(write_half),
        };
        let reader = ChannelReader {
            rx: out_rx,
            buf: Vec::new(),
            pos: 0,
        };
        let finish = Box::new(move || {
            exit_rx
                .recv()
                .map_err(|_| io::Error::other("コマンドの終了を受け取れませんでした"))
        });
        Ok(Process::new(Box::new(writer), Box::new(reader), finish))
    }

    fn is_closed(&self) -> bool {
        self.handle.is_closed()
    }
}

struct ChannelReader {
    rx: mpsc::Receiver<Vec<u8>>,
    buf: Vec<u8>,
    pos: usize,
}

impl Read for ChannelReader {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        while self.pos >= self.buf.len() {
            match self.rx.blocking_recv() {
                Some(b) => {
                    self.buf = b;
                    self.pos = 0;
                }
                None => return Ok(0),
            }
        }
        let n = out.len().min(self.buf.len() - self.pos);
        out[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

type AsyncWriter = Pin<Box<dyn tokio::io::AsyncWrite + Send>>;

struct ChannelWriter {
    inner: Option<AsyncWriter>,
    half: Option<russh::ChannelWriteHalf<client::Msg>>,
}

impl Write for ChannelWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let w = self
            .inner
            .as_mut()
            .ok_or_else(|| io::Error::from(io::ErrorKind::BrokenPipe))?;
        runtime()?.block_on(w.write(buf))
    }

    fn flush(&mut self) -> io::Result<()> {
        match self.inner.as_mut() {
            Some(w) => runtime()?.block_on(w.flush()),
            None => Ok(()),
        }
    }
}

impl Drop for ChannelWriter {
    fn drop(&mut self) {
        // 相手に EOF を送る（エージェントはこれで終わる）
        let (Some(mut w), Some(half)) = (self.inner.take(), self.half.take()) else {
            return;
        };
        if let Ok(rt) = runtime() {
            rt.spawn(async move {
                let _ = w.flush().await;
                let _ = half.eof().await;
            });
        }
    }
}
