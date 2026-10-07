//! 模擬ホストを起動する（x3270・yyterm から接続して確かめる）。
//!
//! ```text
//! yy-3270-mock [--port 3270] [--no-tn3270e] [--mvs38] [--ccsid 930] [--no-responses]
//!              [--tls | --starttls] [--self-signed] [--require-client-cert] [--cert-dir フォルダ]
//! ```
//!
//! 出来事は標準エラーに出す。ログオンのパスワードは `SECRET`。
//!
//! TLS では、試験用の証明書（認証局・サーバー・自己署名・クライアント）を作って `--cert-dir`
//! （既定 `yy-3270-mock-certs`）に書く（`ca.pem`・`client.pem`・`client.key` などを端末側で使う）。
//! サーバーの証明書は、認証局が発行したもの（`--self-signed` なら自己署名）。

use std::time::Duration;

use yy_3270_mock::tls::{TestPki, TlsMode};
use yy_3270_mock::{IndFileStyle, MockConfig, MockHost};
use yy_encoding::Ccsid;

fn main() {
    let mut cfg = MockConfig::default();
    let mut port = 3270u16;
    let (mut mode, mut self_signed, mut client_cert) = (None, false, false);
    let mut cert_dir = std::path::PathBuf::from("yy-3270-mock-certs");
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--port" => port = args.next().and_then(|p| p.parse().ok()).expect("--port"),
            "--no-tn3270e" => cfg.tn3270e = false,
            "--mvs38" => cfg.ind_file = IndFileStyle::Mvs38,
            "--no-responses" => cfg.responses = false,
            "--tls" => mode = Some(TlsMode::Implicit),
            "--starttls" => mode = Some(TlsMode::StartTls),
            "--self-signed" => self_signed = true,
            "--require-client-cert" => client_cert = true,
            "--cert-dir" => cert_dir = args.next().expect("--cert-dir").into(),
            "--ccsid" => {
                cfg.ccsid = args
                    .next()
                    .and_then(|c| c.parse().ok())
                    .and_then(Ccsid::from_number)
                    .expect("--ccsid")
            }
            other => {
                eprintln!("不明な引数: {other}");
                std::process::exit(2);
            }
        }
    }
    cfg.verbose = true;
    if let Some(mode) = mode {
        let pki = TestPki::generate();
        pki.write_to(&cert_dir).expect("証明書を書けません");
        eprintln!("[mock] certificates in {}", cert_dir.display());
        cfg.tls = Some(pki.tls(mode, self_signed, client_cert));
    }
    let host = MockHost::bind(&format!("127.0.0.1:{port}"), cfg).expect("待ち受けできません");
    eprintln!("[mock] listening on {}", host.addr());
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}
