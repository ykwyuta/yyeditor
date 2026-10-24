use crate::Command;
use std::{
    io::{self, Write},
    path::Path,
};
use yy_merge_core::{
    Cancellation, Result,
    remote::{HostKeyCheck, HostKeyQuestion, InputPrompter},
};

struct ConsolePrompter;
fn answer(label: &str, secret: bool) -> Option<String> {
    if !std::io::IsTerminal::is_terminal(&io::stdin()) {
        return None;
    }
    eprint!("{label}: ");
    let _ = io::stderr().flush();
    #[cfg(windows)]
    let restore = unsafe {
        use windows::Win32::System::Console::*;
        let h = GetStdHandle(STD_INPUT_HANDLE).ok()?;
        let mut mode = CONSOLE_MODE(0);
        GetConsoleMode(h, &mut mode).ok()?;
        if secret {
            SetConsoleMode(h, mode & !ENABLE_ECHO_INPUT).ok()?;
        }
        (h, mode)
    };
    #[cfg(unix)]
    let restore = unsafe {
        let mut mode: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(0, &mut mode) != 0 {
            return None;
        }
        let mut hidden = mode;
        hidden.c_lflag &= !libc::ECHO;
        if secret {
            libc::tcsetattr(0, libc::TCSANOW, &hidden);
        }
        mode
    };
    let mut line = String::new();
    let read = io::stdin().read_line(&mut line);
    #[cfg(windows)]
    unsafe {
        let _ = windows::Win32::System::Console::SetConsoleMode(restore.0, restore.1);
    }
    #[cfg(unix)]
    unsafe {
        libc::tcsetattr(0, libc::TCSANOW, &restore);
    }
    if secret {
        eprintln!();
    }
    read.ok()?;
    Some(line.trim_end_matches(['\r', '\n']).to_owned())
}
impl InputPrompter for ConsolePrompter {
    fn keyboard_interactive(
        &self,
        host: &str,
        name: &str,
        instruction: &str,
        prompts: &[(String, bool)],
    ) -> Option<Vec<String>> {
        eprintln!("{host}: {name} {instruction}");
        prompts
            .iter()
            .map(|(prompt, echo)| answer(prompt, !echo))
            .collect()
    }
    fn confirm_host_key(&self, q: &HostKeyQuestion) -> bool {
        if matches!(q.check, HostKeyCheck::Changed { .. }) {
            eprintln!("SSH host key changed: {}", q.host);
            return false;
        }
        answer(
            &format!(
                "SSH {}:{} {} {} — trust? type yes",
                q.host, q.port, q.algorithm, q.fingerprint
            ),
            false,
        )
        .as_deref()
            == Some("yes")
    }
    fn password(&self, label: &str) -> Option<String> {
        answer(label, true)
    }
    fn passphrase(&self, key: &Path) -> Option<String> {
        answer(&format!("Passphrase {}", key.display()), true)
    }
}

pub fn prepare(
    command: &mut Command,
    cancel: &Cancellation,
) -> Result<Vec<yy_merge_core::remote::Download>> {
    let outputs: Vec<&std::path::PathBuf> = match &*command {
        Command::Merge { output, .. } => vec![output],
        Command::Diff { report, .. }
        | Command::LargeDiff { report, .. }
        | Command::PdfDiff { report, .. } => report.iter().collect(),
        Command::ImageDiff {
            report, diff_image, ..
        } => report.iter().chain(diff_image.iter()).collect(),
        _ => vec![],
    };
    if outputs
        .iter()
        .any(|p| p.to_str().is_some_and(|s| s.starts_with("ssh:")))
    {
        return Err("output must be a local path; SSH inputs are read-only".into());
    }
    let mut downloads = Vec::new();
    let paths = match command {
        Command::Diff { left, right, .. }
        | Command::LargeDiff { left, right, .. }
        | Command::ImageDiff { left, right, .. }
        | Command::PdfDiff { left, right, .. } => vec![left, right],
        Command::Merge {
            base, left, right, ..
        } => vec![base, left, right],
        _ => vec![],
    };
    for path in paths {
        if let Some(download) = yy_merge_core::remote::download(path, &ConsolePrompter, cancel)? {
            *path = download.path.clone();
            downloads.push(download);
        }
    }
    Ok(downloads)
}
