//! yyterm の実行ファイル（ターミナル。12 章）。
#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
fn main() {
    // yyterm [フォルダ | ssh://接続先/パス | ユーザー@ホスト]
    let initial = std::env::args().nth(1);
    if let Err(e) = yy_win::run_terminal(initial, ssh()) {
        eprintln!("yyterm: {e}");
        std::process::exit(1);
    }
}

/// SSH の接続の実装（OpenSSH は使わない。11 章 4）。
#[cfg(all(windows, feature = "ssh"))]
fn ssh() -> Option<yy_win::ConnectorFactory> {
    Some(yy_ssh::SshConnector::factory())
}

#[cfg(all(windows, not(feature = "ssh")))]
fn ssh() -> Option<yy_win::ConnectorFactory> {
    None
}

#[cfg(not(windows))]
fn main() {
    eprintln!("yyterm は Windows 専用です。端末の中核（yy-term）は `cargo test` で検証できます。");
    std::process::exit(1);
}
