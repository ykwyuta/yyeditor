#![cfg_attr(windows, windows_subsystem = "windows")]
fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    #[cfg(windows)]
    if let Err(error) = yy3_ui::run(&args) {
        eprintln!("YY3Designer: {error}");
        std::process::exit(1);
    }
    #[cfg(not(windows))]
    {
        let _ = args;
        eprintln!("YY3Designer GUI requires Windows. The yy3-core library is portable.");
        std::process::exit(2);
    }
}
