#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod recording;
#[cfg(windows)]
mod ui;

fn main() {
    #[cfg(windows)]
    if let Err(e) = ui::run() {
        eprintln!("yytoolbox: {e}");
        std::process::exit(1);
    }
    #[cfg(not(windows))]
    eprintln!(
        "yytoolbox の画面はWindows専用です。計算・変換は cargo test -p yytoolbox で検証できます。"
    );
}
