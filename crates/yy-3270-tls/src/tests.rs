use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::JoinHandle;

use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, Issuer, KeyPair};
use rustls::pki_types::PrivatePkcs8KeyDer;
use rustls::server::WebPkiClientVerifier;
use rustls::{ServerConfig, ServerConnection, StreamOwned};

use super::*;

/// 試験の認証局・サーバー・クライアントの証明書。
struct Pki {
    ca_pem: String,
    ca_der: CertificateDer<'static>,
    issuer: Issuer<'static, KeyPair>,
}

impl Pki {
    fn new() -> Pki {
        let key = KeyPair::generate().unwrap();
        let mut p = CertificateParams::new(Vec::<String>::new()).unwrap();
        p.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        p.distinguished_name.push(DnType::CommonName, "yy test CA");
        let cert = p.self_signed(&key).unwrap();
        Pki {
            ca_pem: cert.pem(),
            ca_der: cert.der().clone(),
            issuer: Issuer::new(p, key),
        }
    }

    /// 認証局が発行した証明書と鍵。
    fn issue(&self, names: &[&str], cn: &str) -> (CertificateDer<'static>, KeyPair, String) {
        let key = KeyPair::generate().unwrap();
        let mut p = CertificateParams::new(names.iter().map(|s| s.to_string()).collect::<Vec<_>>())
            .unwrap();
        p.distinguished_name.push(DnType::CommonName, cn);
        let cert = p.signed_by(&key, &self.issuer).unwrap();
        (cert.der().clone(), key, cert.pem())
    }
}

/// 自己署名の証明書と鍵。
fn self_signed(names: &[&str]) -> (CertificateDer<'static>, KeyPair, String) {
    let key = KeyPair::generate().unwrap();
    let mut p =
        CertificateParams::new(names.iter().map(|s| s.to_string()).collect::<Vec<_>>()).unwrap();
    p.distinguished_name
        .push(DnType::CommonName, "mvs01 self-signed");
    p.distinguished_name
        .push(DnType::OrganizationName, "テスト");
    let cert = p.self_signed(&key).unwrap();
    (cert.der().clone(), key, cert.pem())
}

fn server_config(
    cert: CertificateDer<'static>,
    key: &KeyPair,
    client_ca: Option<&CertificateDer<'static>>,
) -> Arc<ServerConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let b = ServerConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .unwrap();
    let b = match client_ca {
        Some(ca) => {
            let mut roots = RootCertStore::empty();
            roots.add(ca.clone()).unwrap();
            b.with_client_cert_verifier(
                WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider)
                    .build()
                    .unwrap(),
            )
        }
        None => b.with_no_client_auth(),
    };
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der()));
    Arc::new(b.with_single_cert(vec![cert], key).unwrap())
}

/// 1 回だけ受け付ける TLS のサーバー（5 バイトを読み、大文字にして返す）。
/// 終わったらクライアント証明書の主体を返す。
fn serve_once(cfg: Arc<ServerConfig>, starttls: bool) -> (u16, JoinHandle<io::Result<String>>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let h = std::thread::spawn(move || {
        let (mut tcp, _) = l.accept()?;
        tcp.set_read_timeout(Some(Duration::from_secs(10)))?;
        if starttls {
            tcp.write_all(&[IAC, DO, OPT_START_TLS])?;
            let mut b = [0u8; 9];
            tcp.read_exact(&mut b)?;
            assert_eq!(
                b,
                [
                    IAC,
                    WILL,
                    OPT_START_TLS,
                    IAC,
                    SB,
                    OPT_START_TLS,
                    FOLLOWS,
                    IAC,
                    SE
                ]
            );
            tcp.write_all(&[IAC, SB, OPT_START_TLS, FOLLOWS, IAC, SE])?;
        }
        let conn = ServerConnection::new(cfg).map_err(io::Error::other)?;
        let mut s = StreamOwned::new(conn, tcp);
        let mut b = [0u8; 5];
        s.read_exact(&mut b)?;
        s.write_all(&b.to_ascii_uppercase())?;
        s.flush()?;
        let peer = s
            .conn
            .peer_certificates()
            .and_then(|c| c.first())
            .map(|c| CertInfo::from_der(c).subject)
            .unwrap_or_default();
        s.conn.send_close_notify();
        let _ = s.flush();
        Ok(peer)
    });
    (port, h)
}

fn opts(host: &str, port: u16) -> Options {
    Options {
        native_roots: false,
        timeout: Duration::from_secs(10),
        ..Options::new(host, port)
    }
}

/// 呼ばれた回数を数え、決まった答えを返す。
fn confirm(answer: bool) -> (Confirm, Arc<AtomicUsize>, Arc<Mutex<Vec<CertQuestion>>>) {
    let n = Arc::new(AtomicUsize::new(0));
    let qs = Arc::new(Mutex::new(Vec::new()));
    let (n2, qs2) = (n.clone(), qs.clone());
    (
        Arc::new(move |q: &CertQuestion| {
            n2.fetch_add(1, Ordering::SeqCst);
            qs2.lock().unwrap().push(q.clone());
            answer
        }),
        n,
        qs,
    )
}

fn echo(t: &mut TlsStream) -> String {
    t.writer.write_all(b"hello").unwrap();
    let mut b = [0u8; 5];
    t.reader.read_exact(&mut b).unwrap();
    String::from_utf8_lossy(&b).into_owned()
}

fn tcp(port: u16) -> TcpStream {
    TcpStream::connect(("127.0.0.1", port)).unwrap()
}

#[test]
fn reads_certificate_details() {
    let (der, _, _) = self_signed(&["mvs01.example", "127.0.0.1"]);
    let info = CertInfo::from_der(&der);
    assert_eq!(info.subject, "CN=mvs01 self-signed, O=テスト");
    assert_eq!(info.issuer, info.subject);
    assert!(info.self_signed());
    assert_eq!(info.names, vec!["mvs01.example", "127.0.0.1"]);
    assert!(info.not_before.ends_with(" UTC"), "{}", info.not_before);
    assert!(info.not_after.starts_with("4096-"), "{}", info.not_after);
    assert_eq!(info.sha256.len(), 32 * 3 - 1);
    assert_eq!(info.sha256, fingerprint(&der));
}

#[test]
fn self_signed_is_confirmed_once_and_remembered() {
    let dir = tempfile::tempdir().unwrap();
    let known = dir.path().join("known_certs");
    let (cert, key, _) = self_signed(&["localhost"]);
    let cfg = server_config(cert, &key, None);
    let (c, n, qs) = confirm(true);

    let (port, h) = serve_once(cfg.clone(), false);
    let o = Options {
        known_certs: Some(known.clone()),
        ..opts("localhost", port)
    };
    let mut t = connect(tcp(port), false, &o, c.clone()).unwrap();
    assert_eq!(echo(&mut t), "HELLO");
    assert_eq!(t.info.trust, Trust::Accepted);
    assert!(t.info.problem.as_deref().unwrap().contains("自己署名"));
    assert_eq!(t.info.cert.subject, "CN=mvs01 self-signed, O=テスト");
    assert!(t.info.version.starts_with("TLS 1."));
    h.join().unwrap().unwrap();
    assert_eq!(n.load(Ordering::SeqCst), 1);
    assert_eq!(qs.lock().unwrap()[0].check, CertCheck::Unknown);
    let text = std::fs::read_to_string(&known).unwrap();
    assert!(
        text.contains(&format!("localhost:{port} sha256:")),
        "{text}"
    );

    // 2 回目は尋ねない（同じポートの記録を書き直してから）
    let (port2, h) = serve_once(cfg, false);
    std::fs::write(
        &known,
        text.replace(&format!(":{port} "), &format!(":{port2} ")),
    )
    .unwrap();
    let o = Options {
        known_certs: Some(known.clone()),
        ..opts("localhost", port2)
    };
    let mut t = connect(tcp(port2), false, &o, c).unwrap();
    assert_eq!(echo(&mut t), "HELLO");
    assert_eq!(t.info.trust, Trust::Remembered);
    h.join().unwrap().unwrap();
    assert_eq!(n.load(Ordering::SeqCst), 1);
}

#[test]
fn refused_certificate_is_not_recorded() {
    let dir = tempfile::tempdir().unwrap();
    let known = dir.path().join("known_certs");
    let (cert, key, _) = self_signed(&["localhost"]);
    let (port, _h) = serve_once(server_config(cert, &key, None), false);
    let (c, n, _) = confirm(false);
    let o = Options {
        known_certs: Some(known.clone()),
        ..opts("localhost", port)
    };
    let e = connect(tcp(port), false, &o, c).err().unwrap();
    assert_eq!(e.kind(), io::ErrorKind::PermissionDenied);
    assert!(e.to_string().contains("受け入れなかった"), "{e}");
    assert_eq!(n.load(Ordering::SeqCst), 1);
    assert!(!known.exists());
}

#[test]
fn changed_certificate_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let known = dir.path().join("known_certs");
    let (cert, key, _) = self_signed(&["localhost"]);
    let (port, _h) = serve_once(server_config(cert, &key, None), false);
    std::fs::write(
        &known,
        format!("# test\nlocalhost:{port} sha256:00:11:22 CN=old\n"),
    )
    .unwrap();
    // 尋ねても（「はい」でも）接続しない
    let (c, n, qs) = confirm(true);
    let o = Options {
        known_certs: Some(known.clone()),
        ..opts("localhost", port)
    };
    let e = connect(tcp(port), false, &o, c).err().unwrap();
    assert!(e.to_string().contains("記録"), "{e}");
    assert_eq!(n.load(Ordering::SeqCst), 1);
    assert!(matches!(
        &qs.lock().unwrap()[0].check,
        CertCheck::Changed { line: 2, recorded, .. } if recorded == "00:11:22"
    ));
    assert_eq!(
        std::fs::read_to_string(&known).unwrap().lines().count(),
        2,
        "記録は書き換えない"
    );
}

#[test]
fn certificate_from_a_trusted_ca_is_accepted_without_asking() {
    let dir = tempfile::tempdir().unwrap();
    let pki = Pki::new();
    let ca = dir.path().join("ca.pem");
    std::fs::write(&ca, &pki.ca_pem).unwrap();
    let (cert, key, _) = pki.issue(&["localhost"], "localhost");
    let cfg = server_config(cert, &key, None);

    let (port, h) = serve_once(cfg.clone(), false);
    let (c, n, _) = confirm(false);
    let o = Options {
        ca_file: Some(ca.clone()),
        ..opts("localhost", port)
    };
    let mut t = connect(tcp(port), false, &o, c).unwrap();
    assert_eq!(echo(&mut t), "HELLO");
    assert_eq!(t.info.trust, Trust::Verified);
    assert_eq!(t.info.cert.issuer, "CN=yy test CA");
    h.join().unwrap().unwrap();
    assert_eq!(n.load(Ordering::SeqCst), 0);

    // 名前が合わなければ（IP アドレスで接続）尋ねる
    let (port, _h) = serve_once(cfg, false);
    let (c, _, qs) = confirm(false);
    let o = Options {
        ca_file: Some(ca),
        ..opts("127.0.0.1", port)
    };
    assert!(connect(tcp(port), false, &o, c).is_err());
    let p = qs.lock().unwrap()[0].problem.clone();
    assert!(
        p.contains("名前（localhost）") && p.contains("127.0.0.1"),
        "{p}"
    );
}

#[test]
fn client_certificate_is_sent_when_required() {
    let dir = tempfile::tempdir().unwrap();
    let pki = Pki::new();
    let ca = dir.path().join("ca.pem");
    std::fs::write(&ca, &pki.ca_pem).unwrap();
    let (cert, key, _) = pki.issue(&["localhost"], "localhost");
    let cfg = server_config(cert, &key, Some(&pki.ca_der));

    // 証明書なし: 断られる
    let (port, h) = serve_once(cfg.clone(), false);
    let o = Options {
        ca_file: Some(ca.clone()),
        ..opts("localhost", port)
    };
    let (c, _, _) = confirm(false);
    let err = match connect(tcp(port), false, &o, c.clone()) {
        Err(e) => e,
        // TLS 1.3 では、断られたことが読むときに分かる（書かずに読む。読まれないデータを残して
        // サーバーが閉じると、Windows では RST で警告（alert）ごと捨てられる。TN3270 でも端末は
        // ホストの交渉を待ってから書く）
        Ok(mut t) => {
            let mut b = [0u8; 5];
            t.reader.read_exact(&mut b).err().unwrap()
        }
    };
    assert!(err.to_string().contains("クライアント証明書"), "{err}");
    assert!(h.join().unwrap().is_err());

    // 証明書と鍵を 1 つの PEM に
    let (_, ckey, cpem) = pki.issue(&[], "TSOUSER1");
    let both = dir.path().join("client.pem");
    std::fs::write(&both, format!("{cpem}{}", ckey.serialize_pem())).unwrap();
    let (port, h) = serve_once(cfg, false);
    let o = Options {
        ca_file: Some(ca),
        client_cert: Some(both),
        ..opts("localhost", port)
    };
    let mut t = connect(tcp(port), false, &o, c).unwrap();
    assert_eq!(echo(&mut t), "HELLO");
    assert!(t.info.client_cert);
    assert_eq!(h.join().unwrap().unwrap(), "CN=TSOUSER1");
}

#[test]
fn encrypted_or_missing_client_key_is_explained() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("enc.pem");
    let (_, _, pem) = self_signed(&["x"]);
    std::fs::write(
        &p,
        format!(
            "{pem}-----BEGIN ENCRYPTED PRIVATE KEY-----\nAAAA\n-----END ENCRYPTED PRIVATE KEY-----\n"
        ),
    )
    .unwrap();
    let e = load_client_cert(&p, None).err().unwrap();
    assert!(e.to_string().contains("暗号化"), "{e}");
    let e = load_client_cert(&dir.path().join("none.pem"), None)
        .err()
        .unwrap();
    assert!(e.to_string().contains("読めません"), "{e}");
}

#[test]
fn starttls_is_negotiated_before_the_handshake() {
    let (cert, key, _) = self_signed(&["localhost"]);
    let (port, h) = serve_once(server_config(cert, &key, None), true);
    let (c, _, _) = confirm(true);
    let mut t = connect(tcp(port), true, &opts("localhost", port), c).unwrap();
    assert_eq!(echo(&mut t), "HELLO");
    h.join().unwrap().unwrap();
}

#[test]
fn starttls_not_offered_is_reported() {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let _h = std::thread::spawn(move || {
        let (mut s, _) = l.accept().unwrap();
        // TN3270E だけを申し出て、答えを待つ
        s.write_all(&[IAC, DO, 40]).unwrap();
        std::thread::sleep(Duration::from_secs(8));
    });
    let (c, _, _) = confirm(true);
    let started = Instant::now();
    let e = connect(tcp(port), true, &opts("localhost", port), c)
        .err()
        .unwrap();
    assert_eq!(e.kind(), io::ErrorKind::Unsupported);
    assert!(e.to_string().contains("STARTTLS"), "{e}");
    assert!(started.elapsed() < Duration::from_secs(6));
}

#[test]
fn plain_telnet_on_a_tls_connection_is_explained() {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let _h = std::thread::spawn(move || {
        let (mut s, _) = l.accept().unwrap();
        s.write_all(&[IAC, DO, 40]).unwrap();
        std::thread::sleep(Duration::from_secs(3));
    });
    let (c, _, _) = confirm(true);
    let e = connect(tcp(port), false, &opts("localhost", port), c)
        .err()
        .unwrap();
    assert!(e.to_string().contains("STARTTLS"), "{e}");
}

#[test]
fn tls12_only_server() {
    // 古いホスト（TLS 1.2 まで）
    let (cert, key, _) = self_signed(&["localhost"]);
    let cfg =
        ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_protocol_versions(&[&rustls::version::TLS12])
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(
                vec![cert],
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
            )
            .unwrap();
    let (port, h) = serve_once(Arc::new(cfg), false);
    let (c, _, _) = confirm(true);
    let mut t = connect(tcp(port), false, &opts("localhost", port), c).unwrap();
    assert_eq!(echo(&mut t), "HELLO");
    assert_eq!(t.info.version, "TLS 1.2");
    h.join().unwrap().unwrap();
}

#[test]
fn parses_security() {
    assert_eq!(Security::parse("TLS"), Some(Security::Tls));
    assert_eq!(Security::parse("starttls"), Some(Security::StartTls));
    assert_eq!(Security::parse(""), Some(Security::None));
    assert_eq!(Security::parse("ssl3"), None);
}
