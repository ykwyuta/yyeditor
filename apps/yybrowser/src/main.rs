//! yybrowser の実行ファイル（OS と別のプロキシを指定できるタブブラウザ。19 章）。
#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
fn main() {
    // yybrowser [--profile <名前>] [--proxy <URL|direct|system>] [URL...]
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(e) = yy_win::run_browser(args, dev_cert()) {
        eprintln!("yybrowser: {e}");
        std::process::exit(1);
    }
}

/// 開発者用証明書を作る関数（rcgen + ring。19 章 3.6）。
#[cfg(all(windows, feature = "devcert"))]
fn dev_cert() -> Option<yy_win::DevCertFn> {
    Some(yy_browser::rules::generate_dev_cert)
}

#[cfg(all(windows, not(feature = "devcert")))]
fn dev_cert() -> Option<yy_win::DevCertFn> {
    None
}

#[cfg(not(windows))]
fn main() {
    eprintln!(
        "yybrowser は Windows 専用です。中核（yy-browser）は `cargo test -p yy-browser` で検証できます。"
    );
    std::process::exit(1);
}
