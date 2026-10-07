//! TLS（M6）を模擬ホストで試す: 暗黙の TLS・STARTTLS、自己署名の証明書の TOFU、認証局での検証、
//! クライアント証明書。TLS の上で TN3270E・プリンター・IND$FILE が動くことも確かめる。

use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use yy_3270::{Config, Event, Mode};
use yy_3270_macro::Options;
use yy_3270_macro::tcp::TcpHost;
use yy_3270_mock::tls::{TestPki, TlsMode, pem_fingerprint};
use yy_3270_mock::{MockConfig, MockHost};
use yy_3270_tls::{CertCheck, CertQuestion, Confirm, Trust};
use yy_encoding::Ccsid;

const T: Duration = Duration::from_secs(10);

fn pki() -> &'static TestPki {
    static P: OnceLock<TestPki> = OnceLock::new();
    P.get_or_init(TestPki::generate)
}

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("yy3270-tls-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    pki().write_to(&d).unwrap();
    d
}

fn mock(mode: TlsMode, self_signed: bool, client_cert: bool) -> MockHost {
    MockHost::start(MockConfig {
        tls: Some(pki().tls(mode, self_signed, client_cert)),
        ..MockConfig::default()
    })
    .unwrap()
}

fn counting(answer: bool) -> (Confirm, Arc<AtomicUsize>) {
    let n = Arc::new(AtomicUsize::new(0));
    let n2 = n.clone();
    (
        Arc::new(move |q: &CertQuestion| {
            assert_eq!(q.check, CertCheck::Unknown);
            n2.fetch_add(1, Ordering::SeqCst);
            answer
        }),
        n,
    )
}

/// TLS で接続して、画面なしのセッションを始める。
fn connect(
    m: &MockHost,
    starttls: bool,
    opts: yy_3270_tls::Options,
    confirm: Confirm,
    f: impl FnOnce(&mut Config),
) -> std::io::Result<(Arc<TcpHost>, yy_3270_tls::SessionInfo)> {
    let sock = TcpStream::connect(m.addr())?;
    let t = yy_3270_tls::connect(sock, starttls, &opts, confirm)?;
    let mut cfg = Config {
        ccsid: Ccsid::Ibm930,
        ..Config::default()
    };
    f(&mut cfg);
    let h = TcpHost::start(Box::new(t.reader), Box::new(t.writer), cfg);
    h.set_password(|name| (name == "mock").then(|| "SECRET".to_owned()));
    Ok((h, t.info))
}

fn opts(m: &MockHost, d: &Path) -> yy_3270_tls::Options {
    yy_3270_tls::Options {
        native_roots: false,
        known_certs: Some(d.join("known_certs")),
        timeout: T,
        ..yy_3270_tls::Options::new("localhost", m.port())
    }
}

fn run(h: &Arc<TcpHost>, d: &Path, script: &str) {
    yy_3270_macro::run(
        script,
        h.clone(),
        Options {
            out_dir: d.to_path_buf(),
            timeout: 10,
        },
    )
    .unwrap();
}

const LOGON: &str = r#"
wait_unlocked();
type("IBMUSER"); tab(); password("mock"); tab(); type("山田太郎"); tab(); type("TLS");
key("Enter");
wait_text("READY");
"#;

const LOGOFF: &str = r#"
type("LOGOFF"); key("Enter"); wait_text("LOGGED OFF");
"#;

fn has(m: &MockHost, s: &str) -> bool {
    m.wait_event(T, |e| e.contains(s)).is_some()
}

#[test]
fn implicit_tls_with_a_self_signed_certificate_is_trusted_on_first_use() {
    let m = mock(TlsMode::Implicit, true, false);
    let d = dir("tofu");
    let (c, n) = counting(true);
    let (h, info) = connect(&m, false, opts(&m, &d), c.clone(), |_| {}).unwrap();
    assert_eq!(info.trust, Trust::Accepted);
    assert_eq!(info.cert.subject, "CN=yy-3270-mock self-signed");
    assert_eq!(
        Some(info.cert.sha256.clone()),
        pem_fingerprint(&pki().self_signed_cert_pem)
    );
    run(&h, &d, &format!("{LOGON}{LOGOFF}"));
    assert!(has(&m, "tls implicit version=TLSv1_3 client=none"));
    assert!(has(
        &m,
        "logon user=IBMUSER password=ok name=山田太郎 note=TLS"
    ));
    assert!(h.events().contains(&Event::Mode(Mode::Tn3270e)));

    // 2 回目は記録から（尋ねない）
    let (_h2, info) = connect(&m, false, opts(&m, &d), c, |_| {}).unwrap();
    assert_eq!(info.trust, Trust::Remembered);
    assert_eq!(n.load(Ordering::SeqCst), 1);

    // 断れば接続しない
    let d2 = dir("tofu-refuse");
    let (no, _) = counting(false);
    assert!(connect(&m, false, opts(&m, &d2), no, |_| {}).is_err());
}

#[test]
fn starttls_with_a_ca_and_a_client_certificate() {
    let m = mock(TlsMode::StartTls, false, true);
    let d = dir("client");
    let (c, n) = counting(false);
    let o = yy_3270_tls::Options {
        ca_file: Some(d.join("ca.pem")),
        client_cert: Some(d.join("client.pem")),
        client_key: Some(d.join("client.key")),
        ..opts(&m, &d)
    };
    let (h, info) = connect(&m, true, o, c, |_| {}).unwrap();
    assert_eq!(info.trust, Trust::Verified);
    assert_eq!(n.load(Ordering::SeqCst), 0);
    run(&h, &d, &format!("{LOGON}{LOGOFF}"));
    let fp = pem_fingerprint(&pki().client_cert_pem).unwrap();
    assert!(
        has(&m, &format!("tls starttls version=TLSv1_3 client={fp}")),
        "{:#?}",
        m.events()
    );
    assert!(has(&m, "logoff lu=TCP0000"));
}

#[test]
fn missing_client_certificate_is_refused() {
    let m = mock(TlsMode::Implicit, false, true);
    let d = dir("noclient");
    let (c, _) = counting(false);
    let o = yy_3270_tls::Options {
        ca_file: Some(d.join("ca.pem")),
        ..opts(&m, &d)
    };
    // TLS 1.3 では、ハンドシェイクの後でホストが断る（切断される）
    match connect(&m, false, o, c, |_| {}) {
        Err(e) => assert!(e.to_string().contains("クライアント証明書"), "{e}"),
        Ok((h, _)) => {
            let start = std::time::Instant::now();
            while h.connected() && start.elapsed() < T {
                std::thread::sleep(Duration::from_millis(20));
            }
            assert!(!h.connected(), "切断されるはず");
        }
    }
    assert!(has(&m, "tls failed"));
    assert!(!m.events().iter().any(|e| e.starts_with("device ")));
}

#[test]
fn printer_and_ind_file_over_starttls() {
    let m = mock(TlsMode::StartTls, false, false);
    let d = dir("ft");
    let o = || yy_3270_tls::Options {
        ca_file: Some(d.join("ca.pem")),
        ..opts(&m, &d)
    };
    let (c, _) = counting(false);
    let (term, _) = connect(&m, true, o(), c.clone(), |_| {}).unwrap();
    let Some(Event::Device(lu)) = term.wait_event(T, |e| matches!(e, Event::Device(_))) else {
        panic!("{:#?}", term.events())
    };
    let (printer, _) = connect(&m, true, o(), c, |cfg| {
        cfg.printer = true;
        cfg.associate = Some(lu.clone());
    })
    .unwrap();
    assert!(
        printer
            .wait_event(T, |e| *e == Event::Mode(Mode::Tn3270e))
            .is_some()
    );
    // 模擬ホストがプリンターを登録するまで待つ（Windows の CI で PRINT が先に届いたことがある）
    assert!(
        m.wait_event(T, |e| e.contains("printer ready")).is_some(),
        "{:#?}",
        m.events()
    );
    let script = format!(
        r#"{LOGON}
        type("PRINT"); key("Enter"); wait_text("SCS"); wait_unlocked();
        let r = transfer_get("'YY.TEST.VB'", "vb.txt");
        if !r.ok {{ throw r.message; }}
        wait_unlocked();
        {LOGOFF}"#
    );
    run(&term, &d, &script);
    assert_eq!(
        std::fs::read_to_string(d.join("vb.txt")).unwrap(),
        "可変長の 1 行目\r\n\r\nSHORT\r\n最後の行 END\r\n"
    );
    assert!(
        printer
            .wait_event(T, |e| matches!(e, Event::PrintJob(_)))
            .is_some()
    );
    assert_eq!(printer.print_jobs()[0].pages[0][0], "請求書　No.0001");
    assert_eq!(
        m.events()
            .iter()
            .filter(|e| e.starts_with("tls starttls"))
            .count(),
        2
    );
}

#[test]
fn starttls_is_reported_when_the_host_does_not_offer_it() {
    let m = MockHost::start(MockConfig::default()).unwrap();
    let d = dir("nostarttls");
    let (c, _) = counting(true);
    let e = connect(&m, true, opts(&m, &d), c, |_| {}).err().unwrap();
    assert!(e.to_string().contains("STARTTLS"), "{e}");
}
