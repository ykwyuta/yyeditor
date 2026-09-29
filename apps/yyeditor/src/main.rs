//! yyeditor の実行ファイル。
#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
fn main() {
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
    let initial = args.first().map(std::path::PathBuf::from);
    if let Err(e) = yy_win::run(initial) {
        // GUI の初期化に失敗した場合はコンソールがないため、ここでは終了コードのみ返す
        eprintln!("yyeditor: {e}");
        std::process::exit(1);
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("yyeditor は Windows 専用です。コアのライブラリは `cargo test` で検証できます。");
    std::process::exit(1);
}
