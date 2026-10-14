//! yysftp の実行ファイル（ファイル転送。13 章）。
#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
fn main() {
    // yysftp [ssh://接続先/パス | ユーザー@ホスト:/パス | ユーザー@ホスト]
    // yysftp --download ssh://接続先/フォルダ --to 保存先のフォルダ（転送の一覧に入れて始める）
    let mut initial = None;
    let (mut download, mut to) = (None, None);
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--download" => download = args.next(),
            "--to" => to = args.next().map(std::path::PathBuf::from),
            _ if initial.is_none() => initial = Some(a),
            _ => {}
        }
    }
    let download = download.zip(to);
    if let Err(e) = yy_win::run_sftp_with(initial, download, ssh()) {
        eprintln!("yysftp: {e}");
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
        "yysftp は Windows 専用です。転送の中核（yy-remote の sftp・scp・xfer）は `cargo test` で検証できます。"
    );
    std::process::exit(1);
}
