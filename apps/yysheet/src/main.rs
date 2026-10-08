//! yysheet の実行ファイル（スプレッドシート。15 章）。
#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
fn main() {
    // yysheet [ファイル（.yys・.csv・.tsv・固定長）| ssh://接続先/パス]
    let initial = std::env::args_os().nth(1).map(std::path::PathBuf::from);
    if let Err(e) = yy_win::run_sheet(initial, ssh()) {
        eprintln!("yysheet: {e}");
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
    eprintln!(
        "yysheet は Windows 専用です。中核（yy-sheet・yy-numfmt）は `cargo test` で検証できます。"
    );
    std::process::exit(1);
}
