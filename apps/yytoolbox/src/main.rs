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
    // The GUI is Windows-only. Elsewhere this binary is the command line (yytoolbox-cli).
    #[cfg(not(windows))]
    std::process::exit(yytoolbox::cli::run(
        std::env::args_os().skip(1).collect(),
        &mut std::io::stdin().lock(),
        &mut std::io::stdout().lock(),
        &mut std::io::stderr().lock(),
    ));
}
