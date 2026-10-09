//! yyfilemanager の実行ファイル（ファイル管理。18 章）。
#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
fn main() {
    // yyfilemanager [--sync <同期ジョブの名前>]
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(e) = yy_win::run_filemanager(args) {
        eprintln!("yyfilemanager: {e}");
        std::process::exit(1);
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!(
        "yyfilemanager は Windows 専用です。中核（yy-files）は `cargo test -p yy-files` で検証できます。"
    );
    std::process::exit(1);
}
