//! 模擬ホストの TLS（暗黙の TLS・Telnet の STARTTLS・クライアント証明書の要求）と、試験用の証明書。

use std::io;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, Issuer, KeyPair};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::WebPkiClientVerifier;
use rustls::{RootCertStore, ServerConfig, ServerConnection, StreamOwned};
use sha2::{Digest, Sha256};

use crate::codes::*;
use crate::{Conn, NEG, Shared, Stream, Unit};

/// Telnet の START_TLS
pub const OPT_START_TLS: u8 = 46;
/// START_TLS の FOLLOWS
pub const FOLLOWS: u8 = 1;

/// TLS の始め方。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TlsMode {
    /// 接続したらすぐ
    Implicit,
    /// `DO START_TLS` を送り、STARTTLS で
    StartTls,
}

/// 模擬ホストの TLS の設定（PEM）。
#[derive(Clone, Debug)]
pub struct MockTls {
    pub mode: TlsMode,
    /// サーバーの証明書（中間の証明書が続いてもよい）
    pub cert_pem: String,
    pub key_pem: String,
    /// あればクライアント証明書を求め、この認証局が発行したものだけを受け入れる
    pub client_ca_pem: Option<String>,
}

/// 試験用の証明書一式（認証局・サーバー・クライアント・自己署名）。
#[derive(Clone, Debug)]
pub struct TestPki {
    pub ca_pem: String,
    /// `localhost`・`127.0.0.1` の、認証局が発行したサーバーの証明書
    pub server_cert_pem: String,
    pub server_key_pem: String,
    /// 自己署名のサーバーの証明書
    pub self_signed_cert_pem: String,
    pub self_signed_key_pem: String,
    /// クライアント証明書（`CN=TSOUSER1`）
    pub client_cert_pem: String,
    pub client_key_pem: String,
}

impl TestPki {
    pub fn generate() -> TestPki {
        let ca_key = KeyPair::generate().expect("鍵");
        let mut p = CertificateParams::new(Vec::<String>::new()).expect("証明書");
        p.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        p.distinguished_name
            .push(DnType::CommonName, "yy-3270-mock CA");
        let ca = p.self_signed(&ca_key).expect("認証局");
        let issuer = Issuer::new(p, ca_key);
        let issue = |names: &[&str], cn: &str| {
            let key = KeyPair::generate().expect("鍵");
            let mut p =
                CertificateParams::new(names.iter().map(|s| s.to_string()).collect::<Vec<_>>())
                    .expect("証明書");
            p.distinguished_name.push(DnType::CommonName, cn);
            let c = p.signed_by(&key, &issuer).expect("証明書");
            (c.pem(), key.serialize_pem())
        };
        let (server_cert_pem, server_key_pem) = issue(&["localhost", "127.0.0.1"], "localhost");
        let (client_cert_pem, client_key_pem) = issue(&[], "TSOUSER1");
        let key = KeyPair::generate().expect("鍵");
        let mut p = CertificateParams::new(vec!["localhost".to_string(), "127.0.0.1".to_string()])
            .expect("証明書");
        p.distinguished_name
            .push(DnType::CommonName, "yy-3270-mock self-signed");
        let ss = p.self_signed(&key).expect("証明書");
        TestPki {
            ca_pem: ca.pem(),
            server_cert_pem,
            server_key_pem,
            self_signed_cert_pem: ss.pem(),
            self_signed_key_pem: key.serialize_pem(),
            client_cert_pem,
            client_key_pem,
        }
    }

    /// フォルダに書く（`ca.pem`・`server.pem`・`server.key`・`self-signed.pem`・`self-signed.key`・
    /// `client.pem`・`client.key`）。
    pub fn write_to(&self, dir: &Path) -> io::Result<()> {
        std::fs::create_dir_all(dir)?;
        for (name, text) in [
            ("ca.pem", &self.ca_pem),
            ("server.pem", &self.server_cert_pem),
            ("server.key", &self.server_key_pem),
            ("self-signed.pem", &self.self_signed_cert_pem),
            ("self-signed.key", &self.self_signed_key_pem),
            ("client.pem", &self.client_cert_pem),
            ("client.key", &self.client_key_pem),
        ] {
            std::fs::write(dir.join(name), text)?;
        }
        Ok(())
    }

    /// `server` か `self_signed` の証明書で TLS の設定を作る。
    pub fn tls(&self, mode: TlsMode, self_signed: bool, require_client_cert: bool) -> MockTls {
        let (cert_pem, key_pem) = if self_signed {
            (&self.self_signed_cert_pem, &self.self_signed_key_pem)
        } else {
            (&self.server_cert_pem, &self.server_key_pem)
        };
        MockTls {
            mode,
            cert_pem: cert_pem.clone(),
            key_pem: key_pem.clone(),
            client_ca_pem: require_client_cert.then(|| self.ca_pem.clone()),
        }
    }
}

/// PEM の最初の証明書の SHA-256 の指紋（`AB:CD:…`）。
pub fn pem_fingerprint(pem: &str) -> Option<String> {
    let der = CertificateDer::from_pem_slice(pem.as_bytes()).ok()?;
    Some(fingerprint(&der))
}

fn fingerprint(der: &[u8]) -> String {
    Sha256::digest(der)
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// rustls のサーバーの設定を作る。
pub(crate) fn server_config(t: &MockTls) -> io::Result<Arc<ServerConfig>> {
    let err = |e: &dyn std::fmt::Display| io::Error::other(format!("TLS の設定: {e}"));
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(t.cert_pem.as_bytes())
        .collect::<Result<_, _>>()
        .map_err(|e| err(&e))?;
    let key = PrivateKeyDer::from_pem_slice(t.key_pem.as_bytes()).map_err(|e| err(&e))?;
    let b = ServerConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|e| err(&e))?;
    let b = match &t.client_ca_pem {
        Some(ca) => {
            let mut roots = RootCertStore::empty();
            for c in CertificateDer::pem_slice_iter(ca.as_bytes()) {
                roots.add(c.map_err(|e| err(&e))?).map_err(|e| err(&e))?;
            }
            b.with_client_cert_verifier(
                WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider)
                    .build()
                    .map_err(|e| err(&e))?,
            )
        }
        None => b.with_no_client_auth(),
    };
    Ok(Arc::new(
        b.with_single_cert(certs, key).map_err(|e| err(&e))?,
    ))
}

/// 受け付けた接続で TLS を始める（STARTTLS なら交渉してから）。
pub(crate) fn accept(
    stream: Box<dyn Stream>,
    mode: TlsMode,
    cfg: &Arc<ServerConfig>,
    shared: &Shared,
) -> io::Result<Conn> {
    let mut raw = match mode {
        TlsMode::Implicit => stream,
        TlsMode::StartTls => {
            let mut c = Conn::new(stream);
            c.send_cmd(DO, OPT_START_TLS)?;
            let (mut will, mut follows) = (false, false);
            while !(will && follows) {
                match c.next(NEG)? {
                    None => {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "STARTTLS の交渉が終わりません",
                        ));
                    }
                    Some(Unit::Cmd(WILL, OPT_START_TLS)) => will = true,
                    Some(Unit::Cmd(WONT, OPT_START_TLS)) => {
                        shared.log("client refused STARTTLS");
                        return Err(io::Error::other("端末が STARTTLS を断りました"));
                    }
                    Some(Unit::Sb(b)) if b == [OPT_START_TLS, FOLLOWS] => follows = true,
                    Some(u) => shared.log(format!("ignored before STARTTLS: {u:?}")),
                }
            }
            c.send_sb(&[OPT_START_TLS, FOLLOWS])?;
            c.into_inner()
        }
    };
    let mut conn = ServerConnection::new(cfg.clone()).map_err(io::Error::other)?;
    let deadline = Instant::now() + NEG;
    while conn.is_handshaking() {
        match conn.complete_io(&mut raw) {
            Ok(_) => {}
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(e) => {
                // 警告（alert）を届ける
                let _ = conn.write_tls(&mut raw);
                return Err(e);
            }
        }
    }
    let client = conn
        .peer_certificates()
        .and_then(|c| c.first())
        .map_or_else(|| "none".to_owned(), |c| fingerprint(c));
    shared.log(format!(
        "tls {} version={} client={client}",
        match mode {
            TlsMode::Implicit => "implicit",
            TlsMode::StartTls => "starttls",
        },
        conn.protocol_version()
            .map(|v| format!("{v:?}"))
            .unwrap_or_default()
    ));
    Ok(Conn::new(Box::new(StreamOwned::new(conn, raw))))
}
