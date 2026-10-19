#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
fn main() {
    let initial = std::env::args_os().nth(1).map(std::path::PathBuf::from);
    if let Err(error) = yy_win::run_draw(initial) {
        eprintln!("yydraw: {error}");
        std::process::exit(1);
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("yydraw は Windows 専用です。yy-draw は他の OS でもテストできます。");
}
