//! yysheet の実行ファイル（スプレッドシート。15 章）。
#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
fn main() {
    // yysheet [ファイル（.yys・.csv・.tsv）]
    let initial = std::env::args_os().nth(1).map(std::path::PathBuf::from);
    if let Err(e) = yy_win::run_sheet(initial) {
        eprintln!("yysheet: {e}");
        std::process::exit(1);
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!(
        "yysheet は Windows 専用です。中核（yy-sheet・yy-numfmt）は `cargo test` で検証できます。"
    );
    std::process::exit(1);
}
