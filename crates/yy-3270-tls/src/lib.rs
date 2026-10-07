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
//! 暗号は rustls（ring。機能 `rustls`）。端末側は OpenSSL・OpenSSH に依存しない。機能 `rustls` なしでは
//! 型と設定だけを使え、[`connect`] はエラーを返す（C のビルドの要らない型の確かめ用）。
//!
//! つないだ後の読み書きは [`TlsReader`]・[`TlsWriter`] に分かれ、別々のスレッドで使える（端末の
//! 読み取りのスレッドと書き込み）。

use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

mod cert;
#[cfg(feature = "rustls")]
mod client;
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
    pub reader: Box<dyn Read + Send>,
    pub writer: Box<dyn Write + Send>,
    pub info: SessionInfo,
    /// 下の TCP（`shutdown` で読み取りのスレッドを止める）
    pub socket: TcpStream,
}

/// Telnet の START_TLS
pub const OPT_START_TLS: u8 = 46;
/// START_TLS の FOLLOWS
pub const FOLLOWS: u8 = 1;

/// TCP の接続の上で TLS を始める（`starttls` なら Telnet の STARTTLS を交渉してから）。
pub fn connect(
    sock: TcpStream,
    starttls: bool,
    opts: &Options,
    confirm: Confirm,
) -> io::Result<TlsStream> {
    #[cfg(feature = "rustls")]
    {
        client::connect(sock, starttls, opts, confirm)
    }
    #[cfg(not(feature = "rustls"))]
    {
        let _ = (sock, starttls, opts, confirm);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "この版には TLS が組み込まれていません",
        ))
    }
}

#[cfg(all(test, feature = "rustls"))]
mod tests;
