//! 3270 の TLS（14 章 M6）。
//!
//! - 暗黙の TLS（`tn3270s://`、ふつうはポート 992）: 接続したらすぐ TLS のハンドシェイクをする。
//! - Telnet の STARTTLS（オプション 46）: ホストの `DO START_TLS` に `WILL START_TLS` と
//!   `SB START_TLS FOLLOWS` で答え、ホストの `FOLLOWS` を受けてからハンドシェイクをする。それまでの
//!   Telnet の交渉は捨てる（TLS の後でホストが交渉し直す）。
//! - サーバーの証明書: OS の信頼する認証局（と設定の CA のファイル）で検証する。検証できなければ
//!   （自己署名など）、SSH のホスト鍵と同じく利用者に確かめて記録する（TOFU）。記録と違えば接続しない。
//! - クライアント証明書: PEM のファイル（証明書と秘密鍵）。
//!
//! 暗号は rustls（ring）。端末側は OpenSSL・OpenSSH に依存しない。
//!
//! つないだ後の読み書きは [`TlsReader`]・[`TlsWriter`] に分かれ、別々のスレッドで使える（端末の
//! 読み取りのスレッドと書き込み）。

use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rustls::client::WebPkiServerVerifier;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::{
    AlertDescription, CertificateError, ClientConfig, ClientConnection, DigitallySignedStruct,
    RootCertStore, SignatureScheme,
};

mod cert;
pub mod known;

pub use cert::{CertInfo, fingerprint};

/// 接続の守り方。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Security {
    /// TLS を使わない
    #[default]
    None,
    /// 暗黙の TLS（接続したらすぐ）
    Tls,
    /// Telnet の STARTTLS
    StartTls,
}

impl Security {
    /// 設定の値（`none`・`tls`・`starttls`）を読む。
    pub fn parse(s: &str) -> Option<Security> {
        match s.trim().to_ascii_lowercase().as_str() {
            "" | "none" | "off" | "no" => Some(Security::None),
            "tls" | "implicit" | "on" | "yes" => Some(Security::Tls),
            "starttls" => Some(Security::StartTls),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Security::None => "none",
            Security::Tls => "tls",
            Security::StartTls => "starttls",
        }
    }
}

/// TLS の設定。
#[derive(Clone, Debug)]
pub struct Options {
    /// 接続先の名前（証明書の名前と照らす。IP アドレスでもよい）
    pub host: String,
    pub port: u16,
    /// 加えて信頼する認証局の証明書（PEM）
    pub ca_file: Option<PathBuf>,
    /// OS の信頼する認証局を使う
    pub native_roots: bool,
    /// クライアント証明書（PEM。秘密鍵も入っていてよい）
    pub client_cert: Option<PathBuf>,
    /// クライアント証明書の秘密鍵（PEM。なければ `client_cert` から読む）
    pub client_key: Option<PathBuf>,
    /// 受け入れた証明書の記録（なければ毎回確かめる）
    pub known_certs: Option<PathBuf>,
    /// ハンドシェイク・STARTTLS の交渉を待つ時間
    pub timeout: Duration,
}

impl Options {
    pub fn new(host: &str, port: u16) -> Options {
        Options {
            host: host.to_owned(),
            port,
            ca_file: None,
            native_roots: true,
            client_cert: None,
            client_key: None,
            known_certs: None,
            timeout: Duration::from_secs(30),
        }
    }
}

/// 検証できない証明書を記録と照らした結果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CertCheck {
    /// 記録がない（初めて）
    Unknown,
    /// 記録と違う
    Changed {
        file: PathBuf,
        line: usize,
        recorded: String,
    },
}

/// 利用者への問い（検証できない証明書を受け入れるか）。
#[derive(Clone, Debug)]
pub struct CertQuestion {
    pub host: String,
    pub port: u16,
    pub cert: CertInfo,
    /// 検証できなかった理由
    pub problem: String,
    pub check: CertCheck,
}

/// 問いに答える（`true` で受け入れる）。[`CertCheck::Changed`] では知らせるだけ（答えによらず断る）。
pub type Confirm = Arc<dyn Fn(&CertQuestion) -> bool + Send + Sync>;

/// 証明書を信頼した理由。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Trust {
    /// 認証局で検証できた
    Verified,
    /// 記録にあった（以前に受け入れた）
    Remembered,
    /// 利用者がいま受け入れた
    Accepted,
}

/// つないだ TLS の情報。
#[derive(Clone, Debug)]
pub struct SessionInfo {
    /// `TLS 1.3` など
    pub version: String,
    /// 暗号スイート
    pub cipher: String,
    /// サーバーの証明書
    pub cert: CertInfo,
    pub trust: Trust,
    /// 検証できなかった理由（[`Trust::Verified`] 以外）
    pub problem: Option<String>,
    /// クライアント証明書を用意した
    pub client_cert: bool,
}

impl SessionInfo {
    /// 1 行の説明（記録用）。
    pub fn summary(&self) -> String {
        let trust = match self.trust {
            Trust::Verified => "認証局で検証",
            Trust::Remembered => "記録にある証明書",
            Trust::Accepted => "利用者が受け入れた証明書",
        };
        format!(
            "{} {}、{trust}、{}{}",
            self.version,
            self.cipher,
            self.cert.subject,
            if self.client_cert {
                "、クライアント証明書あり"
            } else {
                ""
            }
        )
    }
}

/// TLS でつないだ接続。
pub struct TlsStream {
    pub reader: TlsReader,
    pub writer: TlsWriter,
    pub info: SessionInfo,
    /// 下の TCP（`shutdown` で読み取りのスレッドを止める）
    pub socket: TcpStream,
}

/// TCP の接続の上で TLS を始める（`starttls` なら Telnet の STARTTLS を交渉してから）。
pub fn connect(
    mut sock: TcpStream,
    starttls: bool,
    opts: &Options,
    confirm: Confirm,
) -> io::Result<TlsStream> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let outcome = Arc::new(Mutex::new(Outcome::default()));
    let verifier = Verifier::new(opts, provider.clone(), confirm, outcome.clone())?;
    let builder = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(io::Error::other)?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier));
    let client_cert = opts.client_cert.is_some();
    let config = match &opts.client_cert {
        Some(cert) => {
            let (chain, key) = load_client_cert(cert, opts.client_key.as_deref())?;
            builder
                .with_client_auth_cert(chain, key)
                .map_err(|e| bad(format!("クライアント証明書を使えません: {e}")))?
        }
        None => builder.with_no_client_auth(),
    };
    let name = ServerName::try_from(opts.host.trim_matches(['[', ']']).to_owned())
        .map_err(|_| bad(format!("接続先の名前（{}）を TLS で使えません", opts.host)))?;
    let mut conn = ClientConnection::new(Arc::new(config), name).map_err(io::Error::other)?;

    sock.set_read_timeout(Some(opts.timeout))?;
    if starttls {
        negotiate_starttls(&mut sock, opts.timeout)?;
    }
    while conn.is_handshaking() {
        if let Err(e) = conn.complete_io(&mut sock) {
            return Err(handshake_error(e, &outcome, starttls));
        }
    }
    sock.set_read_timeout(None)?;

    let o = outcome.lock().unwrap().clone();
    let info = SessionInfo {
        version: match conn.protocol_version() {
            Some(rustls::ProtocolVersion::TLSv1_3) => "TLS 1.3".into(),
            Some(rustls::ProtocolVersion::TLSv1_2) => "TLS 1.2".into(),
            Some(v) => format!("{v:?}"),
            None => "TLS".into(),
        },
        cipher: conn
            .negotiated_cipher_suite()
            .map(|s| format!("{:?}", s.suite()))
            .unwrap_or_default(),
        cert: o.cert.unwrap_or_default(),
        trust: o.trust.unwrap_or(Trust::Verified),
        problem: o.problem,
        client_cert,
    };
    let socket = sock.try_clone()?;
    let shared = Arc::new(Shared {
        conn: Mutex::new(conn),
        out: sock.try_clone()?,
    });
    Ok(TlsStream {
        reader: TlsReader {
            shared: shared.clone(),
            sock,
            pending: Vec::new(),
        },
        writer: TlsWriter { shared },
        info,
        socket,
    })
}

fn bad(msg: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, msg)
}

/// クライアント証明書（PEM）を読む。
fn load_client_cert(
    cert: &Path,
    key: Option<&Path>,
) -> io::Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>)> {
    let read_err =
        |p: &Path, e: &dyn std::fmt::Display| bad(format!("{} を読めません: {e}", p.display()));
    let chain: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(cert)
        .map_err(|e| read_err(cert, &e))?
        .collect::<Result<_, _>>()
        .map_err(|e| read_err(cert, &e))?;
    if chain.is_empty() {
        return Err(bad(format!(
            "{} に証明書（-----BEGIN CERTIFICATE-----）がありません",
            cert.display()
        )));
    }
    let key_path = key.unwrap_or(cert);
    let key = PrivateKeyDer::from_pem_file(key_path).map_err(|e| {
        let text = std::fs::read_to_string(key_path).unwrap_or_default();
        if text.contains("ENCRYPTED") {
            bad(format!(
                "{} の秘密鍵は暗号化されています。暗号化していない PEM にしてください \
                 （例: openssl pkey -in 鍵 -out 新しい鍵）",
                key_path.display()
            ))
        } else {
            read_err(key_path, &e)
        }
    })?;
    Ok((chain, key))
}

// ---- STARTTLS ----------------------------------------------------------------------

const IAC: u8 = 255;
const DONT: u8 = 254;
const DO: u8 = 253;
const WONT: u8 = 252;
const WILL: u8 = 251;
const SB: u8 = 250;
const SE: u8 = 240;
/// Telnet の START_TLS
pub const OPT_START_TLS: u8 = 46;
/// START_TLS の FOLLOWS
pub const FOLLOWS: u8 = 1;

/// ホストの `DO START_TLS` を待って答え、`FOLLOWS` を受けるまで交渉する。
fn negotiate_starttls(sock: &mut TcpStream, timeout: Duration) -> io::Result<()> {
    #[derive(Clone, Copy)]
    enum Rx {
        Data,
        Iac,
        Cmd(u8),
        Sb,
        SbIac,
    }
    let not_offered = || {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "接続先は STARTTLS を申し出ませんでした（TLS を使わないポートか、暗黙の TLS のポートの\
             おそれがあります）",
        )
    };
    let deadline = Instant::now() + timeout;
    sock.set_read_timeout(Some(Duration::from_millis(200)))?;
    let (mut rx, mut sb, mut sent) = (Rx::Data, Vec::new(), false);
    // ほかの交渉が来て静かになったら、STARTTLS はないとみなす
    let mut other_since: Option<Instant> = None;
    let mut b = [0u8; 1];
    loop {
        if Instant::now() >= deadline {
            return Err(if sent {
                io::Error::new(
                    io::ErrorKind::TimedOut,
                    "STARTTLS の交渉が終わりません（ホストが FOLLOWS を送りません）",
                )
            } else {
                not_offered()
            });
        }
        let n = match sock.read(&mut b) {
            Ok(n) => n,
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                if !sent && other_since.is_some_and(|t| t.elapsed() > Duration::from_secs(3)) {
                    return Err(not_offered());
                }
                continue;
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "STARTTLS の交渉中に切断されました",
            ));
        }
        let c = b[0];
        rx = match (rx, c) {
            (Rx::Data, IAC) => Rx::Iac,
            (Rx::Data, _) => Rx::Data,
            (Rx::Iac, SB) => {
                sb.clear();
                Rx::Sb
            }
            (Rx::Iac, DO | DONT | WILL | WONT) => Rx::Cmd(c),
            (Rx::Iac, _) => Rx::Data,
            (Rx::Cmd(cmd), opt) => {
                match (cmd, opt) {
                    (DO, OPT_START_TLS) if !sent => {
                        sock.write_all(&[
                            IAC,
                            WILL,
                            OPT_START_TLS,
                            IAC,
                            SB,
                            OPT_START_TLS,
                            FOLLOWS,
                            IAC,
                            SE,
                        ])?;
                        sent = true;
                    }
                    (DONT | WONT, OPT_START_TLS) => {
                        return Err(io::Error::new(
                            io::ErrorKind::Unsupported,
                            "接続先が STARTTLS を断りました",
                        ));
                    }
                    (DO | WILL, _) => {
                        other_since.get_or_insert_with(Instant::now);
                    }
                    _ => {}
                }
                Rx::Data
            }
            (Rx::Sb, IAC) => Rx::SbIac,
            (Rx::Sb, _) => {
                sb.push(c);
                Rx::Sb
            }
            (Rx::SbIac, SE) => {
                if sb == [OPT_START_TLS, FOLLOWS] {
                    if !sent {
                        sock.write_all(&[IAC, SB, OPT_START_TLS, FOLLOWS, IAC, SE])?;
                    }
                    sock.set_read_timeout(Some(timeout))?;
                    return Ok(());
                }
                other_since.get_or_insert_with(Instant::now);
                Rx::Data
            }
            (Rx::SbIac, IAC) => {
                sb.push(IAC);
                Rx::Sb
            }
            (Rx::SbIac, _) => Rx::Data,
        };
    }
}

// ---- 証明書の検証 ------------------------------------------------------------------

#[derive(Clone, Default)]
struct Outcome {
    cert: Option<CertInfo>,
    trust: Option<Trust>,
    problem: Option<String>,
    /// 断った理由
    refused: Option<String>,
}

struct Verifier {
    inner: Option<Arc<WebPkiServerVerifier>>,
    /// 認証局の一覧を作れなかった理由
    no_roots: Option<String>,
    algorithms: WebPkiSupportedAlgorithms,
    host: String,
    port: u16,
    known: Option<PathBuf>,
    confirm: Confirm,
    outcome: Arc<Mutex<Outcome>>,
}

impl std::fmt::Debug for Verifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Verifier")
            .field("host", &self.host)
            .field("port", &self.port)
            .finish()
    }
}

impl Verifier {
    fn new(
        opts: &Options,
        provider: Arc<CryptoProvider>,
        confirm: Confirm,
        outcome: Arc<Mutex<Outcome>>,
    ) -> io::Result<Verifier> {
        let mut roots = RootCertStore::empty();
        if opts.native_roots {
            let r = rustls_native_certs::load_native_certs();
            let _ = roots.add_parsable_certificates(r.certs);
        }
        if let Some(ca) = &opts.ca_file {
            let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(ca)
                .and_then(|it| it.collect::<Result<_, _>>())
                .map_err(|e| bad(format!("CA のファイル {} を読めません: {e}", ca.display())))?;
            let (added, _) = roots.add_parsable_certificates(certs);
            if added == 0 {
                return Err(bad(format!(
                    "CA のファイル {} に使える証明書がありません",
                    ca.display()
                )));
            }
        }
        let (inner, no_roots) = if roots.is_empty() {
            (None, Some("信頼する認証局の一覧が空です".to_owned()))
        } else {
            match WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider.clone())
                .build()
            {
                Ok(v) => (Some(v), None),
                Err(e) => (None, Some(format!("認証局の一覧を使えません: {e}"))),
            }
        };
        Ok(Verifier {
            inner,
            no_roots,
            algorithms: provider.signature_verification_algorithms,
            host: opts.host.clone(),
            port: opts.port,
            known: opts.known_certs.clone(),
            confirm,
            outcome,
        })
    }

    fn refuse(&self, why: String) -> rustls::Error {
        self.outcome.lock().unwrap().refused = Some(why.clone());
        rustls::Error::General(why)
    }
}

const SELF_SIGNED: &str = "自己署名の証明書です（信頼する認証局が発行したものではありません）";

/// 検証できなかった理由（利用者向け）。
fn describe_problem(e: &rustls::Error, host: &str, info: &CertInfo) -> String {
    match e {
        rustls::Error::InvalidCertificate(c) => match c {
            CertificateError::UnknownIssuer if info.self_signed() => SELF_SIGNED.into(),
            CertificateError::UnknownIssuer => {
                format!("発行者（{}）を信頼する認証局までたどれません", info.issuer)
            }
            CertificateError::Expired | CertificateError::ExpiredContext { .. } => {
                format!("有効期限（{}）が切れています", info.not_after)
            }
            CertificateError::NotValidYet | CertificateError::NotValidYetContext { .. } => {
                format!("まだ有効になっていません（{} から）", info.not_before)
            }
            CertificateError::NotValidForName | CertificateError::NotValidForNameContext { .. } => {
                let names = if info.names.is_empty() {
                    info.subject.clone()
                } else {
                    info.names.join(", ")
                };
                format!("証明書の名前（{names}）が接続先（{host}）と合いません")
            }
            CertificateError::Revoked => "失効した証明書です".into(),
            other => format!("証明書を検証できません（{other:?}）"),
        },
        other => format!("証明書を検証できません（{other}）"),
    }
}

impl ServerCertVerifier for Verifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let info = CertInfo::from_der(end_entity.as_ref());
        let problem = match &self.inner {
            Some(v) => {
                match v.verify_server_cert(
                    end_entity,
                    intermediates,
                    server_name,
                    ocsp_response,
                    now,
                ) {
                    Ok(ok) => {
                        let mut o = self.outcome.lock().unwrap();
                        o.cert = Some(info);
                        o.trust = Some(Trust::Verified);
                        return Ok(ok);
                    }
                    Err(e) => describe_problem(&e, &self.host, &info),
                }
            }
            None if info.self_signed() => SELF_SIGNED.into(),
            None => format!(
                "{}（発行者: {}）",
                self.no_roots.as_deref().unwrap_or_default(),
                info.issuer
            ),
        };
        let key = known::key(&self.host, self.port);
        let check = match &self.known {
            Some(path) => match known::lookup(path, &key, &info.sha256) {
                Ok(known::Lookup::Match) => {
                    let mut o = self.outcome.lock().unwrap();
                    o.cert = Some(info);
                    o.trust = Some(Trust::Remembered);
                    o.problem = Some(problem);
                    return Ok(ServerCertVerified::assertion());
                }
                Ok(known::Lookup::Unknown) => CertCheck::Unknown,
                Ok(known::Lookup::Changed { line, recorded }) => CertCheck::Changed {
                    file: path.clone(),
                    line,
                    recorded,
                },
                Err(e) => {
                    return Err(
                        self.refuse(format!("証明書の記録 {} を読めません: {e}", path.display()))
                    );
                }
            },
            None => CertCheck::Unknown,
        };
        let q = CertQuestion {
            host: self.host.clone(),
            port: self.port,
            cert: info.clone(),
            problem: problem.clone(),
            check: check.clone(),
        };
        let yes = (self.confirm)(&q);
        if let CertCheck::Changed { file, line, .. } = &check {
            return Err(self.refuse(format!(
                "{key} の証明書が記録（{} の {line} 行目）と違うため、接続しません。\
                 受け取った証明書の指紋: {}",
                file.display(),
                info.sha256
            )));
        }
        if !yes {
            return Err(self.refuse(format!(
                "証明書を受け入れなかったため、接続しません（{problem}）"
            )));
        }
        if let Some(path) = &self.known
            && let Err(e) = known::add(path, &key, &info.sha256, &info.subject)
        {
            return Err(self.refuse(format!("証明書の記録 {} に書けません: {e}", path.display())));
        }
        let mut o = self.outcome.lock().unwrap();
        o.cert = Some(info);
        o.trust = Some(Trust::Accepted);
        o.problem = Some(problem);
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

/// ハンドシェイクの失敗を利用者向けの言葉にする。
fn handshake_error(e: io::Error, outcome: &Mutex<Outcome>, starttls: bool) -> io::Error {
    if let Some(why) = outcome.lock().unwrap().refused.clone() {
        return io::Error::new(io::ErrorKind::PermissionDenied, why);
    }
    let tls = e
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<rustls::Error>());
    let msg = match tls {
        Some(t) => describe_tls(t, starttls),
        None if matches!(
            e.kind(),
            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
        ) =>
        {
            "TLS のハンドシェイクが終わりません（接続先が答えません）".into()
        }
        None if e.kind() == io::ErrorKind::UnexpectedEof => {
            "TLS のハンドシェイク中に切断されました（クライアント証明書が要る接続先かもしれません）"
                .into()
        }
        None => format!("TLS のハンドシェイクに失敗しました: {e}"),
    };
    io::Error::new(e.kind(), msg)
}

/// TLS の誤りを利用者向けの言葉にする。
fn describe_tls(t: &rustls::Error, starttls: bool) -> String {
    match t {
        rustls::Error::AlertReceived(
            AlertDescription::CertificateRequired
            | AlertDescription::BadCertificate
            | AlertDescription::UnknownCA
            | AlertDescription::CertificateUnknown
            | AlertDescription::AccessDenied,
        ) => format!(
            "接続先がクライアント証明書を受け入れませんでした（{t}）。クライアント証明書が要るか、\
             設定の証明書を接続先が信頼していません"
        ),
        rustls::Error::AlertReceived(AlertDescription::HandshakeFailure) => format!(
            "TLS のハンドシェイクに失敗しました（{t}）。クライアント証明書が要るか、使える暗号・\
             TLS の版（1.2 以上）が合いません"
        ),
        rustls::Error::InvalidMessage(_) if !starttls => format!(
            "接続先が TLS で答えませんでした（{t}）。TLS のポートでないか、STARTTLS を使う接続先の\
             おそれがあります"
        ),
        _ => format!("TLS の誤り: {t}"),
    }
}

// ---- 読み書き ----------------------------------------------------------------------

struct Shared {
    conn: Mutex<ClientConnection>,
    /// 書き込み用の TCP（`conn` の鍵を持って書く）
    out: TcpStream,
}

impl Shared {
    fn flush_tls(&self, c: &mut ClientConnection) -> io::Result<()> {
        while c.wants_write() {
            c.write_tls(&mut &self.out)?;
        }
        Ok(())
    }
}

/// TLS の読み口（平文を返す。0 で終わり）。
pub struct TlsReader {
    shared: Arc<Shared>,
    sock: TcpStream,
    pending: Vec<u8>,
}

impl Read for TlsReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            {
                let mut c = self.shared.conn.lock().unwrap();
                match c.reader().read(buf) {
                    Ok(n) => return Ok(n),
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                    // close_notify なしに切れた
                    Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(0),
                    Err(e) => return Err(e),
                }
                if !self.pending.is_empty() {
                    let n = c.read_tls(&mut &self.pending[..])?;
                    self.pending.drain(..n);
                    if let Err(e) = c.process_new_packets() {
                        // 届いていない警告（alert）があれば送ってから
                        let _ = self.shared.flush_tls(&mut c);
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            describe_tls(&e, false),
                        ));
                    }
                    self.shared.flush_tls(&mut c)?;
                    continue;
                }
            }
            let mut tmp = [0u8; 16 * 1024];
            let n = loop {
                match self.sock.read(&mut tmp) {
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    r => break r?,
                }
            };
            if n == 0 {
                return Ok(0);
            }
            self.pending.extend_from_slice(&tmp[..n]);
        }
    }
}

/// TLS の書き口。
pub struct TlsWriter {
    shared: Arc<Shared>,
}

impl Write for TlsWriter {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        let mut c = self.shared.conn.lock().unwrap();
        let mut rest = b;
        while !rest.is_empty() {
            let n = c.writer().write(rest)?;
            self.shared.flush_tls(&mut c)?;
            if n == 0 {
                return Err(io::ErrorKind::WriteZero.into());
            }
            rest = &rest[n..];
        }
        Ok(b.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut c = self.shared.conn.lock().unwrap();
        self.shared.flush_tls(&mut c)
    }
}

impl TlsWriter {
    /// close_notify を送る（閉じる前に）。
    pub fn close(&mut self) -> io::Result<()> {
        let mut c = self.shared.conn.lock().unwrap();
        c.send_close_notify();
        self.shared.flush_tls(&mut c)
    }
}

#[cfg(test)]
mod tests;
