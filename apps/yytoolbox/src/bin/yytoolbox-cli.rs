//! Console entry point of yytoolbox. Runs the same tools as the Windows GUI
//! from a terminal, scripts or CI, and counts source code steps.
fn main() {
    let args = std::env::args_os().skip(1).collect();
    let code = yytoolbox::cli::run(
        args,
        &mut std::io::stdin().lock(),
        &mut std::io::stdout().lock(),
        &mut std::io::stderr().lock(),
    );
    std::process::exit(code);
}
