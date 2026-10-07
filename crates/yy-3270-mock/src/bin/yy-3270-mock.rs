//! 模擬ホストを起動する（x3270・yyterm から接続して確かめる）。
//!
//! ```text
//! yy-3270-mock [--port 3270] [--no-tn3270e] [--mvs38] [--ccsid 930] [--no-responses]
//! ```
//!
//! 出来事は標準エラーに出す。ログオンのパスワードは `SECRET`。

use std::time::Duration;

use yy_3270_mock::{IndFileStyle, MockConfig, MockHost};
use yy_encoding::Ccsid;

fn main() {
    let mut cfg = MockConfig::default();
    let mut port = 3270u16;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--port" => port = args.next().and_then(|p| p.parse().ok()).expect("--port"),
            "--no-tn3270e" => cfg.tn3270e = false,
            "--mvs38" => cfg.ind_file = IndFileStyle::Mvs38,
            "--no-responses" => cfg.responses = false,
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
    let host = MockHost::bind(&format!("127.0.0.1:{port}"), cfg).expect("待ち受けできません");
    eprintln!("[mock] listening on {}", host.addr());
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}
