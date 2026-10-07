//! 画面なしでマクロを動かす（Hercules・模擬ホストなどでの確かめ・調査用）。
//!
//! ```text
//! cargo run -p yy-3270-macro --example run -- ホスト:ポート マクロ.rhai [出力のフォルダ]
//! ```
//!
//! - `password("名前")` は環境変数 `YY3270_PASSWORD_名前`（英大文字）か `YY3270_PASSWORD` の値を入れる。
//! - `YY3270_TRACE=ファイル` で通信の記録を書く。`YY3270_CCSID`（既定 37）・`YY3270_MODEL`（既定 2）・
//!   `YY3270_TN3270E=0` で TN3270 にする。`YY3270_LU` で TN3270E の LU 名を求める。
//! - `YY3270_PRINTER=1` でプリンター（3287）として接続する（`YY3270_ASSOCIATE` で端末の LU に対応づける）。
//! - `YY3270_TLS=tls`（暗黙の TLS）・`starttls` で TLS にする。`YY3270_CA_FILE`・`YY3270_CLIENT_CERT`・
//!   `YY3270_CLIENT_KEY`（PEM）。検証できない証明書は `YY3270_ACCEPT_CERT=1` なら受け入れる（記録しない）。
//! - `log`・`message`・`print_screen` は標準出力に出す。終わったら最後の画面と受け取った印刷を出す。

use std::path::PathBuf;

use yy_3270::Config;
use yy_3270_macro::Host;
use yy_3270_macro::tcp::TcpHost;
use yy_encoding::Ccsid;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("使い方: run ホスト:ポート マクロ.rhai [出力のフォルダ]");
        std::process::exit(2);
    }
    let script = std::fs::read_to_string(&args[2]).expect("マクロを読めません");
    let out_dir = PathBuf::from(args.get(3).map_or("macros-out", String::as_str));
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let env_num = |k: &str, d: u32| env(k).and_then(|v| v.parse().ok()).unwrap_or(d);
    let cfg = Config {
        ccsid: Ccsid::from_number(env_num("YY3270_CCSID", 37)).expect("CCSID"),
        model: env_num("YY3270_MODEL", 2) as u8,
        tn3270e: env("YY3270_TN3270E").is_none_or(|v| v != "0"),
        lu: env("YY3270_LU"),
        printer: env("YY3270_PRINTER").is_some_and(|v| v == "1"),
        associate: env("YY3270_ASSOCIATE"),
        ..Config::default()
    };
    let host = match env("YY3270_TLS").map(|v| yy_3270_tls::Security::parse(&v)) {
        None | Some(Some(yy_3270_tls::Security::None)) => {
            TcpHost::connect(&args[1], cfg).expect("接続できません")
        }
        Some(None) => panic!("YY3270_TLS は tls か starttls"),
        Some(Some(sec)) => {
            let (h, p) = args[1].rsplit_once(':').expect("ホスト:ポート");
            let path = |k: &str| env(k).map(PathBuf::from);
            let opts = yy_3270_tls::Options {
                ca_file: path("YY3270_CA_FILE"),
                client_cert: path("YY3270_CLIENT_CERT"),
                client_key: path("YY3270_CLIENT_KEY"),
                ..yy_3270_tls::Options::new(h, p.parse().expect("ポート"))
            };
            let accept = env("YY3270_ACCEPT_CERT").is_some_and(|v| v == "1");
            let confirm: yy_3270_tls::Confirm = std::sync::Arc::new(move |q| {
                eprintln!(
                    "検証できない証明書（{}）:\n{}",
                    q.problem,
                    q.cert.describe()
                );
                accept
            });
            let sock = std::net::TcpStream::connect(&args[1]).expect("接続できません");
            let t =
                yy_3270_tls::connect(sock, sec == yy_3270_tls::Security::StartTls, &opts, confirm)
                    .expect("TLS でつなげません");
            eprintln!("TLS: {}", t.info.summary());
            TcpHost::start(Box::new(t.reader), Box::new(t.writer), cfg)
        }
    };
    host.set_verbose(true);
    host.set_password(|name| {
        std::env::var(format!("YY3270_PASSWORD_{}", name.to_ascii_uppercase()))
            .or_else(|_| std::env::var("YY3270_PASSWORD"))
            .ok()
    });
    if let Some(p) = env("YY3270_TRACE") {
        host.set_trace(Box::new(std::fs::File::create(p).expect("記録のファイル")));
    }
    let r = yy_3270_macro::run(
        &script,
        host.clone(),
        yy_3270_macro::Options {
            out_dir,
            timeout: 60,
        },
    );
    println!("---- 最後の画面 ----");
    for l in host.snapshot().unwrap().lines() {
        println!("| {}", l.trim_end());
    }
    for job in host.print_jobs() {
        println!("---- 印刷（{} ページ） ----", job.pages.len());
        print!("{}", job.to_text());
    }
    match r {
        Ok(()) => println!("マクロが終わりました"),
        Err(e) => {
            println!("マクロのエラー: {e}");
            std::process::exit(1);
        }
    }
}
