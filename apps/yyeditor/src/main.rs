//! yyeditor の実行ファイル。
#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
fn main() {
    // ソース管理（git）の資格情報を答える askpass として起動された
    if yy_win::git_askpass() {
        return;
    }
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    // 描画確認用: yyeditor --render-bmp <入力> <出力.bmp>
    if args.first().is_some_and(|a| a == "--render-bmp") && args.len() == 3 {
        let r = yy_win::render_to_bmp(args[1].as_ref(), args[2].as_ref(), 1000, 640);
        if let Err(e) = r {
            eprintln!("yyeditor: {e}");
            std::process::exit(1);
        }
        return;
    }
    // yyeditor [--line <行番号>] [ファイル]
    let mut initial = None;
    let mut line = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--line" {
            line = it.next().and_then(|n| n.to_str()?.parse::<u64>().ok());
        } else if initial.is_none() {
            initial = Some(std::path::PathBuf::from(a));
        }
    }
    if let Err(e) = yy_win::run(initial, line, ssh()) {
        // GUI の初期化に失敗した場合はコンソールがないため、ここでは終了コードのみ返す
        eprintln!("yyeditor: {e}");
        std::process::exit(1);
    }
}

/// SSH 接続先のファイルの編集に使う接続の実装（OpenSSH は使わない。11 章 4）。
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
    eprintln!("yyeditor は Windows 専用です。コアのライブラリは `cargo test` で検証できます。");
    std::process::exit(1);
}
