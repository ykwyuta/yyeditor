//! yyeditor に組み込む SSH クライアント（11 章 4）。
//!
//! SSH プロトコルは Pure Rust の `russh`（暗号処理は `ring`）で話し、`ssh.exe` など OpenSSH の
//! プログラムは使わない（C1）。非同期実行（tokio）はこのクレートの中に閉じ込め、外には
//! [`yy_remote::Connector`] と [`yy_remote::Transport`] の同期的な trait だけを見せる。
//!
//! ホスト鍵は、鍵交換（サーバーの署名の検証）が済んだ時点で受け取っておき、認証の情報を
//! 送る前に `known_hosts` と照合する。初めてのホストは利用者に確かめてから記録し、記録と
//! 違う鍵なら認証せずに切断する。
//!
//! 踏み台（ProxyJump）を経由する場合は、踏み台ごとに同じ手順（ホスト鍵の照合と認証）で
//! 接続し、次の接続先へは踏み台の `direct-tcpip` チャネルの上で SSH を話す。最初の接続先
//! （踏み台がなければ接続先そのもの）への TCP 接続には HTTP・SOCKS のプロキシを使える。

mod forward;
mod proxy;

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use russh::client::{self, AuthResult, Handle, KeyboardInteractiveAuthResponse};
use russh::keys::{PrivateKey, PrivateKeyWithHashAlg, PublicKey, PublicKeyOrCertificate};
use russh::{ChannelMsg, MethodKind};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::runtime::Runtime;
use tokio::sync::mpsc;
use yy_remote::known_hosts::{self, HostKeyStatus};
use yy_remote::{
    ConnectLog, Connector, ConnectorFactory, ConnectorOptions, Exit, HostKeyCheck, HostKeyQuestion,
    HostSpec, PassphraseRequest, PasswordAnswer, PasswordRequest, PasswordStore, Process, Prompter,
    STDERR_LIMIT, SavedPassword, Shell, Transport,
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
    /// パスワードの保存先（`None` なら保存しない・使わない）
    pub passwords: Option<Arc<dyn PasswordStore>>,
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
    /// `-R` の振り分け表（接続先から開かれた forwarded-tcpip チャネル）
    routes: forward::Routes,
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

    async fn server_channel_open_forwarded_tcpip(
        &mut self,
        channel: russh::Channel<client::Msg>,
        _connected_address: &str,
        connected_port: u32,
        originator_address: &str,
        originator_port: u32,
        reply: client::ChannelOpenHandle,
        _session: &mut client::Session,
    ) -> Result<(), Self::Error> {
        forward::on_forwarded(
            &self.routes,
            channel,
            connected_port,
            format!("{originator_address}:{originator_port}"),
            reply,
        );
        Ok(())
    }
}

impl SshConnector {
    pub fn new(opts: &ConnectorOptions) -> SshConnector {
        SshConnector {
            known_hosts: opts.known_hosts.clone(),
            extra_known_hosts: opts.extra_known_hosts.clone(),
            keepalive: opts.keepalive,
            passwords: opts.passwords.clone(),
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
        log: &ConnectLog,
    ) -> io::Result<()> {
        let openssh = key
            .to_openssh()
            .map_err(|e| io::Error::other(e.to_string()))?;
        let mut words = openssh.split_whitespace();
        let (Some(algorithm), Some(b64)) = (words.next(), words.next()) else {
            return Err(io::Error::other("ホスト鍵を読めません"));
        };
        let name = spec.known_hosts_name();
        let fingerprint = known_hosts::fingerprint(b64).unwrap_or_default();
        log.note(format!(
            "ホスト鍵: {algorithm} {fingerprint}（known_hosts での名前 {name}）"
        ));
        let files = self.known_hosts_files();
        let status = known_hosts::check(&files, &name, algorithm, b64);
        let check = match status {
            HostKeyStatus::Known => {
                log.note("ホスト鍵は known_hosts の記録と一致しました");
                return Ok(());
            }
            HostKeyStatus::Unknown => {
                let looked: Vec<String> = files.iter().map(|f| f.display().to_string()).collect();
                log.note(format!(
                    "ホスト鍵の記録がありません（{}）。利用者に確かめます",
                    looked.join(", ")
                ));
                HostKeyCheck::Unknown
            }
            HostKeyStatus::Changed { file, line } => {
                log.note(format!(
                    "ホスト鍵が記録（{} の {line} 行目）と違います",
                    file.display()
                ));
                HostKeyCheck::Changed { file, line }
            }
        };
        let question = HostKeyQuestion {
            host: spec.hostname.clone(),
            port: spec.port,
            algorithm: algorithm.to_owned(),
            fingerprint,
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
                log.note(format!(
                    "ホスト鍵を承認しました（{} に記録）",
                    self.known_hosts.display()
                ));
                known_hosts::learn(&self.known_hosts, &name, algorithm, b64)
            }
            HostKeyCheck::Unknown => Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "ホスト鍵を承認しなかったため接続しませんでした",
            )),
        }
    }
}

/// SSH を話す下の接続（TCP・プロキシ経由の TCP・踏み台のチャネル）。
trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}

impl SshConnector {
    /// 1 つのホストに接続し、ホスト鍵を照合して認証する。`via` があればその踏み台を経由する。
    #[allow(clippy::too_many_arguments)]
    fn connect_hop(
        &self,
        rt: &Runtime,
        config: &Arc<client::Config>,
        spec: &HostSpec,
        via: Option<&Handle<Client>>,
        routes: forward::Routes,
        prompter: &dyn Prompter,
        log: &ConnectLog,
    ) -> io::Result<Handle<Client>> {
        let host = spec.hostname.as_str();
        let port = spec.port;
        let proxy = spec.proxy.as_ref().filter(|_| via.is_none());
        let how = match (via, proxy) {
            (Some(_), _) => "踏み台経由".to_owned(),
            (None, Some(p)) => format!("プロキシ {p} 経由"),
            (None, None) => "直接".to_owned(),
        };
        log.note(format!("{} に接続します（{how}）", spec.address()));
        // プロキシを使う場合は、先にプロキシを通した TCP 接続を作る（認証を尋ねてやり直せるように）
        let mut proxied = match proxy {
            Some(p) => Some(connect_proxy(
                rt,
                p,
                host,
                port,
                prompter,
                self.passwords.as_deref(),
                log,
            )?),
            None => None,
        };
        let server_key = Arc::new(Mutex::new(None));
        let handler = Client {
            server_key: server_key.clone(),
            routes,
        };
        let mut handle = rt.block_on(async {
            let connect = async {
                let stream: Box<dyn Stream> = match (via, proxied.take()) {
                    (Some(jump), _) => {
                        let ch = jump
                            .channel_open_direct_tcpip(host, u32::from(port), "127.0.0.1", 0)
                            .await
                            .map_err(|e| {
                                io::Error::new(
                                    io::ErrorKind::ConnectionRefused,
                                    format!("踏み台から {host}:{port} に接続できませんでした: {e}"),
                                )
                            })?;
                        log.note(format!(
                            "踏み台の上で {host}:{port} へのチャネル（direct-tcpip）を開きました"
                        ));
                        Box::new(ch.into_stream())
                    }
                    (None, Some(tcp)) => Box::new(tcp),
                    (None, None) => {
                        let tcp =
                            tokio::net::TcpStream::connect((host, port))
                                .await
                                .map_err(|e| {
                                    io::Error::new(
                                        e.kind(),
                                        format!("{host}:{port} に接続できませんでした: {e}"),
                                    )
                                })?;
                        tcp.set_nodelay(true)?;
                        if let Ok(addr) = tcp.peer_addr() {
                            log.note(format!("TCP で接続しました（{addr}）"));
                        }
                        Box::new(tcp)
                    }
                };
                let handle = client::connect_stream(config.clone(), stream, handler)
                    .await
                    .map_err(|e| {
                        let e = ssh_error(e);
                        io::Error::new(e.kind(), format!("SSH の接続の確立に失敗しました: {e}"))
                    })?;
                log.note("SSH の鍵交換が済みました");
                Ok(handle)
            };
            match tokio::time::timeout(CONNECT_TIMEOUT, connect).await {
                Ok(r) => r,
                Err(_) => Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!(
                        "{host}:{port} に接続できませんでした（{} 秒で時間切れ）",
                        CONNECT_TIMEOUT.as_secs()
                    ),
                )),
            }
        })?;
        let key = server_key
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| io::Error::other("ホスト鍵を受け取れませんでした"))?;
        if let Err(e) = self.verify_host_key(spec, &key, prompter, log) {
            disconnect(rt, &handle);
            return Err(e);
        }
        if let Err(e) = authenticate(
            rt,
            &mut handle,
            spec,
            prompter,
            self.passwords.as_deref(),
            log,
        ) {
            disconnect(rt, &handle);
            return Err(e);
        }
        Ok(handle)
    }
}

impl Connector for SshConnector {
    fn connect(
        &self,
        spec: &HostSpec,
        prompter: &dyn Prompter,
        log: &ConnectLog,
    ) -> io::Result<Arc<dyn Transport>> {
        for l in spec.describe() {
            log.note(l);
        }
        let hops: Vec<&HostSpec> = spec.jumps.iter().chain([spec]).collect();
        if let Some(e) = hops.iter().find_map(|h| h.route_error.as_ref()) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, e.clone()));
        }
        let rt = runtime()?;
        let config = Arc::new(client::Config {
            keepalive_interval: Some(self.keepalive),
            keepalive_max: 3,
            inactivity_timeout: None,
            nodelay: true,
            ..client::Config::default()
        });
        let mut handles: Vec<Arc<Handle<Client>>> = Vec::new();
        // 接続先（最後のホップ）の -R の振り分け表
        let routes = forward::Routes::default();
        for (i, hop) in hops.iter().enumerate() {
            let via = handles.last().map(|h| h.as_ref());
            let r = if i + 1 == hops.len() {
                routes.clone()
            } else {
                forward::Routes::default()
            };
            match self.connect_hop(rt, &config, hop, via, r, prompter, log) {
                Ok(h) => handles.push(Arc::new(h)),
                Err(e) => {
                    for h in handles.iter().rev() {
                        disconnect(rt, h);
                    }
                    // 踏み台で失敗したことが分かるようにする（中止などの種類は保つ）
                    return Err(if i + 1 < hops.len() {
                        io::Error::new(e.kind(), format!("踏み台 {}: {e}", hop.user_host()))
                    } else {
                        e
                    });
                }
            }
        }
        log.note("SSH の接続と認証が済みました");
        let handle = handles.pop().expect("at least one hop");
        Ok(Arc::new(SshTransport {
            handle,
            jumps: handles,
            routes,
        }))
    }
}

/// プロキシの認証の情報と、その出どころ。
struct ProxyCreds {
    user: String,
    password: String,
    from: CredsFrom,
}

#[derive(PartialEq)]
enum CredsFrom {
    /// 設定に書いてあった
    Config,
    /// 保存してあった
    Saved,
    /// 尋ねた（`true` なら成功したら保存する）
    Asked(bool),
}

/// プロキシを通して `host:port` への TCP 接続を作る。プロキシに認証を求められたり、認証に
/// 失敗したりしたら、保存したパスワード、なければユーザー名（設定に書いていなければ）と
/// パスワードを尋ねてやり直す。
fn connect_proxy(
    rt: &Runtime,
    p: &yy_remote::proxy::Proxy,
    host: &str,
    port: u16,
    prompter: &dyn Prompter,
    store: Option<&dyn PasswordStore>,
    log: &ConnectLog,
) -> io::Result<tokio::net::TcpStream> {
    let key = yy_remote::proxy_password_key(p);
    let mut ask = ProxyAsk {
        proxy: p,
        key: &key,
        store,
        prompter,
        log,
        saved_tried: false,
        note: None,
    };
    let mut creds = match (&p.user, &p.password) {
        (Some(u), Some(pw)) => Some(ProxyCreds {
            user: u.clone(),
            password: pw.clone(),
            from: CredsFrom::Config,
        }),
        // ユーザー名だけを書いた場合は、最初からパスワードを用意する
        (Some(u), None) => Some(ask.obtain(Some(u))?),
        _ => None,
    };
    let mut asked = 0;
    loop {
        let c = creds.as_ref().map(|c| proxy::Credentials {
            user: &c.user,
            password: &c.password,
        });
        let r = rt.block_on(async {
            match tokio::time::timeout(CONNECT_TIMEOUT, proxy::connect(p, c, host, port)).await {
                Ok(r) => r,
                Err(_) => Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("プロキシ {p} に接続できませんでした（時間切れ）"),
                )),
            }
        });
        match r {
            Ok(tcp) => {
                log.note(format!(
                    "プロキシ {p} が {host}:{port} への中継を始めました{}",
                    match creds.as_ref().map(|c| &c.from) {
                        None => "",
                        Some(CredsFrom::Saved) => "（保存したパスワードで認証）",
                        Some(_) => "（認証あり）",
                    }
                ));
                if let (Some(c), Some(store)) = (&creds, store)
                    && c.from == CredsFrom::Asked(true)
                {
                    save_password(store, &key, &c.user, &c.password, log);
                }
                return Ok(tcp);
            }
            Err(e) if proxy::needs_credentials(&e) && asked < RETRIES => {
                log.note(format!("{e}"));
                if let (Some(c), Some(store)) = (&creds, store)
                    && c.from == CredsFrom::Saved
                {
                    forget_password(store, &key, log);
                    ask.note = Some(REJECTED_NOTE);
                } else {
                    asked += 1;
                }
                let hint = creds.as_ref().map(|c| c.user.clone()).or(p.user.clone());
                creds = Some(ask.obtain(hint.as_deref())?);
            }
            Err(e) => return Err(e),
        }
    }
}

/// 保存したパスワードが受け付けられなかったときの説明
const REJECTED_NOTE: &str = "保存したパスワードは受け付けられませんでした（保存を削除しました）。";

/// プロキシの認証の情報を用意する。
struct ProxyAsk<'a> {
    proxy: &'a yy_remote::proxy::Proxy,
    key: &'a str,
    store: Option<&'a dyn PasswordStore>,
    prompter: &'a dyn Prompter,
    log: &'a ConnectLog,
    saved_tried: bool,
    note: Option<&'static str>,
}

impl ProxyAsk<'_> {
    /// 保存したパスワード（まだ試していなければ）、なければ尋ねる。
    fn obtain(&mut self, user: Option<&str>) -> io::Result<ProxyCreds> {
        if !self.saved_tried {
            self.saved_tried = true;
            if let Some(saved) = self.store.and_then(|s| s.load(self.key))
                && user.is_none_or(|u| u == saved.user)
            {
                self.log.note(format!(
                    "プロキシ {} のパスワードは保存したもの（ユーザー {}）を使います",
                    self.proxy, saved.user
                ));
                return Ok(ProxyCreds {
                    user: saved.user,
                    password: saved.password,
                    from: CredsFrom::Saved,
                });
            }
        }
        let user = match user {
            Some(u) => u.to_owned(),
            None => {
                let prompts = [("ユーザー名:".to_owned(), true)];
                let answers = self
                    .prompter
                    .keyboard_interactive(
                        &format!("プロキシ {}", self.proxy),
                        "プロキシの認証",
                        "",
                        &prompts,
                    )
                    .ok_or_else(cancelled)?;
                answers.into_iter().next().ok_or_else(cancelled)?
            }
        };
        let label = format!("{user}（プロキシ {}）", self.proxy);
        let a = self
            .prompter
            .ask_password(&PasswordRequest {
                label: &label,
                can_save: self.store.is_some(),
                note: self.note.take(),
            })
            .ok_or_else(cancelled)?;
        Ok(ProxyCreds {
            user,
            password: a.password,
            from: CredsFrom::Asked(a.save),
        })
    }
}

/// 使えたパスワード・パスフレーズを保存する（失敗しても接続は続ける）。
fn save_password(
    store: &dyn PasswordStore,
    key: &str,
    user: &str,
    password: &str,
    log: &ConnectLog,
) {
    let saved = SavedPassword {
        user: user.to_owned(),
        password: password.to_owned(),
    };
    match store.save(key, &saved) {
        Ok(()) => log.note(format!("保存しました（{key}）")),
        Err(e) => log.note(format!("保存できませんでした（{key}）: {e}")),
    }
}

/// 受け付けられなかった保存済みのパスワードを消す。
fn forget_password(store: &dyn PasswordStore, key: &str, log: &ConnectLog) {
    store.delete(key);
    log.note(format!(
        "保存したパスワードが受け付けられなかったため、保存を削除しました（{key}）"
    ));
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
    store: Option<&dyn PasswordStore>,
    log: &ConnectLog,
) -> io::Result<()> {
    let user = spec.user.clone();
    log.note(format!("ユーザー {user} で認証します"));
    // サーバーが受け付ける方法（最初は分からないので、すべて試す）
    let mut methods: Option<Vec<MethodKind>> = None;
    let allowed =
        |m: &Option<Vec<MethodKind>>, k: MethodKind| m.as_ref().is_none_or(|v| v.contains(&k));
    let note = |what: &str, r: &AuthResult, m: &mut Option<Vec<MethodKind>>| {
        if let AuthResult::Failure {
            remaining_methods, ..
        } = r
        {
            *m = Some(remaining_methods.iter().copied().collect());
            log.note(format!(
                "{what}: 受け付けられませんでした（サーバーが受け付ける方法: {}）",
                method_names(m)
            ));
        }
    };
    let succeeded = |what: &str| {
        log.note(format!("{what}で認証しました"));
        Ok(())
    };

    for path in &spec.identity_files {
        if !path.is_file() {
            continue;
        }
        if !allowed(&methods, MethodKind::PublicKey) {
            log.note("サーバーが公開鍵認証を受け付けないため、残りの秘密鍵は使いません");
            break;
        }
        let Some(key) = load_key(path, prompter, store, log)? else {
            continue;
        };
        let what = format!("公開鍵（{}、{}）", path.display(), key.algorithm());
        let hash = rt
            .block_on(handle.best_supported_rsa_hash())
            .map_err(ssh_error)?
            .flatten();
        let key = PrivateKeyWithHashAlg::new(Arc::new(key), hash);
        let r = rt
            .block_on(handle.authenticate_publickey(user.clone(), key))
            .map_err(ssh_error)?;
        if r.success() {
            return succeeded(&what);
        }
        note(&what, &r, &mut methods);
    }
    if !spec.identity_files.iter().any(|p| p.is_file()) {
        log.note("使える秘密鍵のファイルがありません");
    }

    if allowed(&methods, MethodKind::KeyboardInteractive) {
        log.note("keyboard-interactive 認証を試します");
        let user_host = spec.user_host();
        let mut r = rt
            .block_on(handle.authenticate_keyboard_interactive_start(user.clone(), None))
            .map_err(ssh_error)?;
        loop {
            match r {
                KeyboardInteractiveAuthResponse::Success => {
                    return succeeded("keyboard-interactive 認証");
                }
                KeyboardInteractiveAuthResponse::Failure {
                    remaining_methods, ..
                } => {
                    methods = Some(remaining_methods.iter().copied().collect());
                    log.note(format!(
                        "keyboard-interactive 認証: 受け付けられませんでした（サーバーが受け付ける方法: {}）",
                        method_names(&methods)
                    ));
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
                        // 質問は記録するが、答えは記録しない
                        let q: Vec<(String, bool)> =
                            prompts.iter().map(|p| (p.prompt.clone(), p.echo)).collect();
                        let asked: Vec<&str> = q.iter().map(|(p, _)| p.trim()).collect();
                        log.note(format!(
                            "keyboard-interactive の質問: {}",
                            asked.join(" / ")
                        ));
                        prompter
                            .keyboard_interactive(&user_host, &name, &instructions, &q)
                            .ok_or_else(|| {
                                log.note("keyboard-interactive の入力が中止されました");
                                cancelled()
                            })?
                    };
                    r = rt
                        .block_on(handle.authenticate_keyboard_interactive_respond(answers))
                        .map_err(ssh_error)?;
                }
            }
        }
    }

    if allowed(&methods, MethodKind::Password) {
        let key = yy_remote::ssh_password_key(spec);
        let mut notice = None;
        // 保存したパスワードがあれば、尋ねずに使う
        if let Some(store) = store
            && let Some(saved) = store.load(&key)
        {
            log.note(format!("保存したパスワードを使います（{key}）"));
            let r = rt
                .block_on(handle.authenticate_password(user.clone(), saved.password))
                .map_err(ssh_error)?;
            if r.success() {
                return succeeded("パスワード認証（保存したパスワード）");
            }
            note("保存したパスワード", &r, &mut methods);
            forget_password(store, &key, log);
            notice = Some(REJECTED_NOTE);
        }
        for i in 1..=RETRIES {
            if !allowed(&methods, MethodKind::Password) {
                break;
            }
            let label = spec.user_host();
            let PasswordAnswer { password, save } = prompter
                .ask_password(&PasswordRequest {
                    label: &label,
                    can_save: store.is_some(),
                    note: notice.take(),
                })
                .ok_or_else(|| {
                    log.note("パスワードの入力が中止されました");
                    cancelled()
                })?;
            let r = rt
                .block_on(handle.authenticate_password(user.clone(), password.clone()))
                .map_err(ssh_error)?;
            if r.success() {
                if save && let Some(store) = store {
                    save_password(store, &key, &user, &password, log);
                }
                return succeeded("パスワード認証");
            }
            note(&format!("パスワード認証（{i} 回目）"), &r, &mut methods);
            if !allowed(&methods, MethodKind::Password) {
                break;
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!(
            "{} に認証できませんでした（サーバーが受け付ける方法: {}）",
            spec.user_host(),
            method_names(&methods)
        ),
    ))
}

/// 認証方式の名前の並び（分からなければ「不明」）。
fn method_names(m: &Option<Vec<MethodKind>>) -> String {
    match m {
        Some(v) if !v.is_empty() => v.iter().map(<&str>::from).collect::<Vec<_>>().join(", "),
        Some(_) => "なし".into(),
        None => "不明".into(),
    }
}

fn cancelled() -> io::Error {
    io::Error::new(io::ErrorKind::Interrupted, "接続を中止しました")
}

/// 秘密鍵を読む。暗号化されていれば、保存したパスフレーズ、なければ尋ねたパスフレーズで開く
/// （答えなければ `None`）。保存を選んだパスフレーズは、鍵を開けたら保存する。
fn load_key(
    path: &Path,
    prompter: &dyn Prompter,
    store: Option<&dyn PasswordStore>,
    log: &ConnectLog,
) -> io::Result<Option<PrivateKey>> {
    match russh::keys::load_secret_key(path, None) {
        Ok(k) => return Ok(Some(k)),
        Err(russh::keys::Error::KeyIsEncrypted) => {}
        // 読めない鍵（対応していない形式など）は使わない
        Err(e) => {
            log.note(format!(
                "秘密鍵 {} を読めないため使いません: {e}",
                path.display()
            ));
            return Ok(None);
        }
    }
    let key = yy_remote::passphrase_key(path);
    let mut notice = None;
    if let Some(store) = store
        && let Some(saved) = store.load(&key)
    {
        if let Ok(k) = russh::keys::load_secret_key(path, Some(&saved.password)) {
            log.note(format!(
                "秘密鍵 {} を保存したパスフレーズで開きました",
                path.display()
            ));
            return Ok(Some(k));
        }
        store.delete(&key);
        log.note(format!(
            "保存したパスフレーズで秘密鍵 {} を開けなかったため、保存を削除しました",
            path.display()
        ));
        notice = Some("保存したパスフレーズでは開けませんでした（保存を削除しました）。");
    }
    for _ in 0..RETRIES {
        let Some(answer) = prompter.ask_passphrase(&PassphraseRequest {
            key_file: path,
            can_save: store.is_some(),
            note: notice.take(),
        }) else {
            log.note(format!(
                "秘密鍵 {} のパスフレーズが入力されなかったため使いません",
                path.display()
            ));
            return Ok(None);
        };
        if let Ok(k) = russh::keys::load_secret_key(path, Some(&answer.password)) {
            if answer.save
                && let Some(store) = store
            {
                save_password(store, &key, "", &answer.password, log);
            }
            return Ok(Some(k));
        }
        log.note(format!(
            "秘密鍵 {} のパスフレーズが違います",
            path.display()
        ));
    }
    Ok(None)
}

/// 認証済みの SSH 接続。
struct SshTransport {
    handle: Arc<Handle<Client>>,
    /// 経由している踏み台の接続（最初の踏み台から順に）
    jumps: Vec<Arc<Handle<Client>>>,
    /// `-R` の振り分け表
    routes: forward::Routes,
}

impl Drop for SshTransport {
    fn drop(&mut self) {
        if let Ok(rt) = runtime() {
            // 接続先から順に、踏み台をさかのぼって切断する
            let handles: Vec<_> = std::iter::once(self.handle.clone())
                .chain(self.jumps.iter().rev().cloned())
                .collect();
            rt.spawn(async move {
                for h in handles {
                    let _ = h
                        .disconnect(russh::Disconnect::ByApplication, "", "en")
                        .await;
                }
            });
        }
    }
}

/// チャネルで始めること。
enum Start {
    Exec(Vec<u8>),
    /// サブシステム（`sftp` など）
    Subsystem(String),
    /// 端末つき（`command` がなければログインシェル）
    Shell {
        term: String,
        cols: u16,
        rows: u16,
        command: Option<Vec<u8>>,
    },
}

/// 開いたチャネル。
struct Opened {
    writer: ChannelWriter,
    reader: ChannelReader,
    finish: Box<dyn FnOnce() -> io::Result<Exit> + Send>,
    half: Arc<russh::ChannelWriteHalf<client::Msg>>,
}

impl SshTransport {
    /// セッションのチャネルを開いて `start` を行う。`merge_stderr` なら標準エラー出力も出力に流す。
    fn open(&self, start: Start, merge_stderr: bool) -> io::Result<Opened> {
        let rt = runtime()?;
        let channel = rt.block_on(async {
            let ch = self.handle.channel_open_session().await?;
            match start {
                Start::Exec(command) => ch.exec(true, command).await?,
                Start::Subsystem(name) => ch.request_subsystem(true, name).await?,
                Start::Shell {
                    term,
                    cols,
                    rows,
                    command,
                } => {
                    ch.request_pty(true, &term, u32::from(cols), u32::from(rows), 0, 0, &[])
                        .await?;
                    match command {
                        Some(c) => ch.exec(true, c).await?,
                        None => ch.request_shell(true).await?,
                    }
                }
            }
            Ok::<_, russh::Error>(ch)
        });
        let channel = channel.map_err(ssh_error)?;
        let (mut read_half, write_half) = channel.split();
        // 出力は数を限った受け渡し口に流す（読む側が遅ければ SSH のウィンドウで送信を止める）
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
                    ChannelMsg::ExtendedData { data, ext: 1 } if merge_stderr => {
                        if let Some(tx) = &out
                            && tx.send(data.to_vec()).await.is_err()
                        {
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
        let half = Arc::new(write_half);
        let writer = ChannelWriter {
            inner: Some(Box::pin(half.make_writer())),
            half: Some(half.clone()),
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
        Ok(Opened {
            writer,
            reader,
            finish,
            half,
        })
    }
}

impl Transport for SshTransport {
    fn exec(&self, command: &[u8]) -> io::Result<Process> {
        let o = self.open(Start::Exec(command.to_vec()), false)?;
        Ok(Process::new(
            Box::new(o.writer),
            Box::new(o.reader),
            o.finish,
        ))
    }

    fn subsystem(&self, name: &str) -> io::Result<Process> {
        let o = self.open(Start::Subsystem(name.to_owned()), false)?;
        Ok(Process::new(
            Box::new(o.writer),
            Box::new(o.reader),
            o.finish,
        ))
    }

    fn shell(&self, term: &str, size: (u16, u16), command: Option<&[u8]>) -> io::Result<Shell> {
        let o = self.open(
            Start::Shell {
                term: term.to_owned(),
                cols: size.0,
                rows: size.1,
                command: command.map(<[u8]>::to_vec),
            },
            true,
        )?;
        let half = o.half;
        let h = half.clone();
        let close = Box::new(move || {
            if let Ok(rt) = runtime() {
                let h = h.clone();
                rt.spawn(async move {
                    let _ = h.close().await;
                });
            }
        });
        let resize = Box::new(move |cols: u16, rows: u16| {
            // 送るだけ（応答は待たない）。入力より先に届くよう、この場で送る
            if let Ok(rt) = runtime() {
                let _ = rt.block_on(half.window_change(u32::from(cols), u32::from(rows), 0, 0));
            }
        });
        Ok(Shell {
            input: Box::new(o.writer),
            output: Box::new(o.reader),
            resize,
            finish: o.finish,
            close,
        })
    }

    fn is_closed(&self) -> bool {
        self.handle.is_closed() || self.jumps.iter().any(|h| h.is_closed())
    }

    fn forward(
        &self,
        f: &yy_remote::Forward,
        note: yy_remote::ForwardNote,
    ) -> io::Result<yy_remote::ActiveForward> {
        forward::start(runtime()?, &self.handle, &self.routes, f, note)
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
    half: Option<Arc<russh::ChannelWriteHalf<client::Msg>>>,
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
