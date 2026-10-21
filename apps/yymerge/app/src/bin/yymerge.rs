fn main() {
    std::process::exit(yy_merge_cli::entry(std::env::args_os(), |g| {
        #[cfg(all(windows, feature = "gui"))]
        {
            let exe = std::env::current_exe()
                .map_err(|e| e.to_string())?
                .with_file_name("yymergew.exe");
            let mut command = std::process::Command::new(exe);
            for (name, value) in [
                ("--left", g.left),
                ("--right", g.right),
                ("--base", g.base),
                ("--output", g.output),
            ] {
                if let Some(value) = value {
                    command.arg(name).arg(value);
                }
            }
            if let Some(e) = g.encoding {
                command.arg("--encoding").arg(e);
            }
            if let Some(d) = g.ui_smoke_test {
                command.arg("--ui-smoke-test").arg(d);
            }
            let mut child = command.spawn().map_err(|e| e.to_string())?;
            if g.wait {
                Ok(child.wait().map_err(|e| e.to_string())?.code().unwrap_or(2))
            } else {
                Ok(0)
            }
        }
        #[cfg(not(all(windows, feature = "gui")))]
        {
            let _ = g;
            Err("GUI requires Windows and the gui feature".into())
        }
    }));
}
