#![cfg_attr(windows, windows_subsystem = "windows")]
fn main() {
    #[cfg(windows)]
    {
        let args = std::iter::once(std::ffi::OsString::from("yymerge"))
            .chain(std::iter::once(std::ffi::OsString::from("gui")))
            .chain(std::env::args_os().skip(1));
        std::process::exit(yy_merge_cli::entry(args, yy_merge_ui::run));
    }
    #[cfg(not(windows))]
    {
        eprintln!("yymergew requires Windows");
        std::process::exit(2);
    }
}
