//! Git owns repository state; this adapter only reads roles and commits a resolved output.
use crate::{Command, Format, Gui};
use clap::Subcommand;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process,
};
use yy_merge_core::{
    Cancellation, Result,
    files::{self, Snapshot},
};

#[derive(Subcommand)]
pub enum GitCommand {
    /// View LOCAL (pre-image) and REMOTE (post-image); wait until the viewer closes.
    Diff {
        #[arg(long)]
        local: PathBuf,
        #[arg(long)]
        remote: PathBuf,
        #[arg(long)]
        encoding: Option<String>,
        #[arg(long)]
        headless: bool,
    },
    /// Merge explicit BASE/LOCAL/REMOTE into MERGED; unresolved/cancelled leaves MERGED intact.
    Merge {
        #[arg(long)]
        base: PathBuf,
        #[arg(long)]
        local: PathBuf,
        #[arg(long)]
        remote: PathBuf,
        #[arg(long)]
        merged: PathBuf,
        #[arg(long)]
        encoding: Option<String>,
        /// Require regular blobs in index stages 1, 2 and 3; reject missing roles.
        #[arg(long)]
        verify_index: bool,
        /// Exact automatic merge only: conflicts return 1 without writing markers.
        #[arg(long)]
        headless: bool,
    },
    /// Print Git configuration; never modify Git settings automatically.
    Config {
        #[arg(long)]
        executable: Option<PathBuf>,
        #[arg(long)]
        headless: bool,
    },
}
fn null_role(path: &Path) -> bool {
    if path == Path::new("/dev/null") {
        return true;
    }
    #[cfg(windows)]
    {
        path.as_os_str()
            .to_str()
            .is_some_and(|s| s.eq_ignore_ascii_case("nul"))
    }
    #[cfg(not(windows))]
    {
        false
    }
}
fn regular(path: &Path) -> Result<()> {
    let m = fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut link = m.file_type().is_symlink();
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        link |= m.file_attributes() & 0x400 != 0;
    }
    #[cfg(not(windows))]
    {
        let _ = &mut link;
    }
    if link || !m.is_file() {
        return Err(format!(
            "Git tool requires a regular file, not a missing role/link/device: {}",
            path.display()
        ));
    }
    Ok(())
}
fn index_has_roles(merged: &Path) -> Result<bool> {
    let output = process::Command::new("git")
        .args(["--literal-pathspecs", "ls-files", "--stage", "-z", "--"])
        .arg(merged)
        .output()
        .map_err(|e| format!("git index verification: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "git index verification: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let mut stages = [false; 3];
    for record in output.stdout.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        let header = record
            .split(|b| *b == b'\t')
            .next()
            .ok_or("invalid Git index record")?;
        let fields: Vec<&[u8]> = header.split(|b| *b == b' ').collect();
        if fields.len() != 3 {
            return Err("invalid Git index record".into());
        }
        if fields[0] != b"100644" && fields[0] != b"100755" {
            return Ok(false);
        }
        let stage = match fields[2] {
            b"1" => 0,
            b"2" => 1,
            b"3" => 2,
            _ => return Ok(false),
        };
        if stages[stage] {
            return Err("Git path matches more than one conflict".into());
        }
        stages[stage] = true;
    }
    Ok(stages == [true; 3])
}
fn output_target(path: &Path, cancel: &Cancellation) -> Result<(PathBuf, Option<Vec<u8>>)> {
    let parent = fs::canonicalize(
        path.parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new(".")),
    )
    .map_err(|e| e.to_string())?;
    let path = parent.join(path.file_name().ok_or("MERGED file name required")?);
    let old = match fs::symlink_metadata(&path) {
        Ok(_) => {
            regular(&path)?;
            Some(files::read_bounded(&path, cancel)?)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.to_string()),
    };
    Ok((path, old))
}
pub fn run(
    command: GitCommand,
    gui: impl FnOnce(Gui) -> Result<i32>,
    stdout: &mut impl Write,
    cancel: &Cancellation,
) -> Result<i32> {
    match command {
        GitCommand::Config {
            executable,
            headless,
        } => {
            let exe = executable
                .map(Ok)
                .unwrap_or_else(std::env::current_exe)
                .map_err(|e| e.to_string())?;
            stdout
                .write_all(configuration(&exe, headless)?.as_bytes())
                .map_err(|e| e.to_string())?;
            Ok(0)
        }
        GitCommand::Diff {
            local,
            remote,
            encoding,
            headless,
        } => {
            let work = tempfile::tempdir().map_err(|e| e.to_string())?;
            let left_missing = null_role(&local);
            let right_missing = null_role(&remote);
            let source = |path: PathBuf, missing: bool, name: &str| -> Result<PathBuf> {
                if missing {
                    let empty = work.path().join(name);
                    fs::write(&empty, []).map_err(|e| e.to_string())?;
                    Ok(empty)
                } else {
                    regular(&path)?;
                    Ok(path)
                }
            };
            let local = source(local, left_missing, "LOCAL-absent")?;
            let remote = source(remote, right_missing, "REMOTE-absent")?;
            if headless && (left_missing || right_missing) {
                writeln!(
                    stdout,
                    "LOCAL: {} / REMOTE: {} (absence is distinct from an empty file)",
                    if left_missing { "absent" } else { "file" },
                    if right_missing { "absent" } else { "file" }
                )
                .map_err(|e| e.to_string())?;
            }
            // A successful viewer is not a failed diff merely because files differ.
            let code = if headless {
                crate::execute(
                    Command::Diff {
                        left: local,
                        right: remote,
                        encoding,
                        format: Format::Text,
                        report: None,
                        overwrite: false,
                        rules: Default::default(),
                    },
                    stdout,
                    cancel,
                )?
            } else {
                gui(Gui {
                    left: Some(local),
                    right: Some(remote),
                    encoding,
                    wait: true,
                    git_diff: true,
                    git_left_missing: left_missing,
                    git_right_missing: right_missing,
                    ..Default::default()
                })?
            };
            Ok(if code == 1 { 0 } else { code })
        }
        GitCommand::Merge {
            base,
            local,
            remote,
            merged,
            encoding,
            verify_index,
            headless,
        } => {
            if verify_index && !index_has_roles(&merged)? {
                writeln!(stdout,"Unresolved: Git index must contain regular BASE/LOCAL/REMOTE blobs (stages 1/2/3). Add/add, deletion, symlink and submodule conflicts require separate handling; MERGED unchanged.").map_err(|e|e.to_string())?;
                return Ok(1);
            }
            for path in [&base, &local, &remote] {
                regular(path)?;
            }
            let b = Snapshot::open(&base, encoding.as_deref(), cancel)?;
            let l = Snapshot::open(&local, encoding.as_deref(), cancel)?;
            let r = Snapshot::open(&remote, encoding.as_deref(), cancel)?;
            if b.encoding != l.encoding
                || b.encoding != r.encoding
                || b.bom != l.bom
                || b.bom != r.bom
            {
                return Err("Git merge inputs must use the same encoding and BOM".into());
            }
            let (target, old) = output_target(&merged, cancel)?;
            if [&b, &l, &r].iter().any(|s| s.path == target) {
                return Err("MERGED must differ from BASE/LOCAL/REMOTE".into());
            }
            let mut recovery = None;
            let bytes = if headless {
                let merge = yy_merge_core::merge(&b.text, &l.text, &r.text, cancel)?;
                if merge.unresolved() > 0 {
                    writeln!(
                        stdout,
                        "{} unresolved conflicts; MERGED unchanged",
                        merge.unresolved()
                    )
                    .map_err(|e| e.to_string())?;
                    return Ok(1);
                }
                b.encode(&merge.render(false)?)?
            } else {
                let work = tempfile::tempdir().map_err(|e| e.to_string())?;
                let staged = work.path().join("resolved-result.txt");
                let code = gui(Gui {
                    left: Some(l.path.clone()),
                    right: Some(r.path.clone()),
                    base: Some(b.path.clone()),
                    output: Some(staged.clone()),
                    encoding,
                    wait: true,
                    git_merge: true,
                    git_merged_target: Some(target.clone()),
                    ..Default::default()
                })?;
                if code != 0 {
                    if staged.is_file() {
                        writeln!(
                            stdout,
                            "MERGED unchanged; saved result retained at: {}",
                            work.keep().join("resolved-result.txt").display()
                        )
                        .map_err(|e| e.to_string())?;
                    }
                    return Ok(code);
                }
                let checked = (|| {
                    regular(&staged)?;
                    let result = Snapshot::open(&staged, Some(b.encoding.name()), cancel)?;
                    if result.encoding != b.encoding || result.bom != b.bom {
                        return Err("resolved result encoding/BOM changed".into());
                    }
                    files::read_bounded(&staged, cancel)
                })();
                match checked {
                    Ok(bytes) => {
                        recovery = Some(work);
                        bytes
                    }
                    Err(e) => return fail_with_result(work, e, stdout),
                }
            };
            if let Err(e) = files::save_if_unchanged(&target, &bytes, &[&b, &l, &r], &old, cancel) {
                return if let Some(work) = recovery {
                    fail_with_result(work, e, stdout)
                } else {
                    Err(e)
                };
            }
            Ok(0)
        }
    }
}
fn fail_with_result(
    work: tempfile::TempDir,
    error: String,
    stdout: &mut impl Write,
) -> Result<i32> {
    let staged = work.path().join("resolved-result.txt");
    if !fs::symlink_metadata(&staged).is_ok_and(|m| m.is_file() && !m.file_type().is_symlink()) {
        return Err(error);
    }
    let cancelled = error == "cancelled";
    let message = format!(
        "{error}; MERGED unchanged; saved result retained at: {}",
        work.keep().join("resolved-result.txt").display()
    );
    if cancelled {
        writeln!(stdout, "{message}").map_err(|e| e.to_string())?;
        Ok(130)
    } else {
        Err(message)
    }
}
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}
fn config_quote(s: &str) -> String {
    format!(
        "\"{}\"",
        s.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\t', "\\t")
    )
}
pub fn configuration(executable: &Path, headless: bool) -> Result<String> {
    let mut path = executable
        .to_str()
        .ok_or("configuration requires a Unicode executable path")?
        .to_owned();
    if path.chars().any(char::is_control) {
        return Err("control characters in executable path".into());
    }
    #[cfg(windows)]
    {
        path = path.replace('\\', "/");
        if let Some(p) = path.strip_prefix("//?/UNC/") {
            path = format!("//{p}")
        } else if let Some(p) = path.strip_prefix("//?/") {
            path = p.to_owned()
        }
    }
    #[cfg(not(windows))]
    {
        let _ = &mut path;
    }
    let exe = shell_quote(&path);
    let mode = if headless { " --headless" } else { "" };
    let diff = format!("{exe} git-tool diff --local=\"$LOCAL\" --remote=\"$REMOTE\"{mode}");
    let merge = format!(
        "{exe} git-tool merge --base=\"$BASE\" --local=\"$LOCAL\" --remote=\"$REMOTE\" --merged=\"$MERGED\" --verify-index{mode}"
    );
    Ok(format!(
        "[diff]\n\ttool = yymerge\n[difftool \"yymerge\"]\n\tcmd = {}\n[merge]\n\ttool = yymerge\n[mergetool \"yymerge\"]\n\tcmd = {}\n\ttrustExitCode = true\n\thideResolved = false\n",
        config_quote(&diff),
        config_quote(&merge)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, [PathBuf; 4]) {
        let tmp = tempfile::tempdir().unwrap();
        let paths = ["BASE", "LOCAL", "REMOTE", "MERGED"].map(|n| tmp.path().join(n));
        for (path, bytes) in paths.iter().zip([
            b"a\nb\n".as_slice(),
            b"A\nb\n",
            b"a\nB\n",
            b"original output",
        ]) {
            fs::write(path, bytes).unwrap();
        }
        (tmp, paths)
    }
    fn merge(p: &[PathBuf; 4], headless: bool) -> GitCommand {
        GitCommand::Merge {
            base: p[0].clone(),
            local: p[1].clone(),
            remote: p[2].clone(),
            merged: p[3].clone(),
            encoding: None,
            verify_index: false,
            headless,
        }
    }
    #[test]
    fn gui_waits_maps_roles_and_defers_original_output_until_resolved() {
        let (_tmp, p) = fixture();
        let c = Cancellation::new();
        let mut out = Vec::new();
        let code = run(
            merge(&p, false),
            |g| {
                assert!(g.wait && g.git_merge);
                assert_eq!(g.base.as_ref().unwrap(), &fs::canonicalize(&p[0]).unwrap());
                assert_eq!(g.left.as_ref().unwrap(), &fs::canonicalize(&p[1]).unwrap());
                assert_eq!(g.right.as_ref().unwrap(), &fs::canonicalize(&p[2]).unwrap());
                assert_eq!(
                    g.git_merged_target.as_ref().unwrap(),
                    &fs::canonicalize(&p[3]).unwrap()
                );
                assert_ne!(g.output.as_ref(), Some(&p[3]));
                assert_eq!(fs::read(&p[3]).unwrap(), b"original output");
                fs::write(g.output.unwrap(), b"resolved by user\n").unwrap();
                Ok(0)
            },
            &mut out,
            &c,
        )
        .unwrap();
        assert_eq!(code, 0);
        assert_eq!(fs::read(&p[3]).unwrap(), b"resolved by user\n");
        assert_eq!(fs::read(&p[1]).unwrap(), b"A\nb\n");
    }
    #[test]
    fn cancelled_missing_result_and_concurrent_changes_never_replace_merged() {
        for condition in ["cancel", "missing", "output", "input", "interrupt"] {
            let (_tmp, p) = fixture();
            let c = Cancellation::new();
            let result = run(
                merge(&p, false),
                |g| {
                    if condition == "cancel" {
                        return Ok(130);
                    }
                    if condition != "missing" {
                        fs::write(g.output.unwrap(), b"resolved\n").unwrap();
                    }
                    if condition == "output" {
                        fs::write(&p[3], b"external edit").unwrap();
                    }
                    if condition == "input" {
                        fs::write(&p[1], b"external edit").unwrap();
                    }
                    if condition == "interrupt" {
                        c.cancel();
                    }
                    Ok(0)
                },
                &mut Vec::new(),
                &c,
            );
            if matches!(condition, "cancel" | "interrupt") {
                assert_eq!(result.unwrap(), 130)
            } else {
                let error = result.unwrap_err();
                if matches!(condition, "output" | "input") {
                    let path = error
                        .split("saved result retained at: ")
                        .nth(1)
                        .expect("resolved result recovery path missing");
                    assert_eq!(fs::read(path).unwrap(), b"resolved\n");
                }
            }
            assert_eq!(
                fs::read(&p[3]).unwrap(),
                if condition == "output" {
                    b"external edit".as_slice()
                } else {
                    b"original output"
                }
            );
        }
    }
    #[test]
    fn headless_conflicts_missing_base_and_input_alias_preserve_output() {
        let (_tmp, p) = fixture();
        let c = Cancellation::new();
        assert_eq!(
            run(merge(&p, true), |_| panic!(), &mut Vec::new(), &c).unwrap(),
            0
        );
        assert_eq!(fs::read(&p[3]).unwrap(), b"A\nB\n");
        fs::write(&p[0], b"").unwrap();
        fs::write(&p[1], b"left\n").unwrap();
        fs::write(&p[2], b"right\n").unwrap();
        assert_eq!(
            run(merge(&p, true), |_| panic!(), &mut Vec::new(), &c).unwrap(),
            1
        );
        assert_eq!(fs::read(&p[3]).unwrap(), b"A\nB\n");
        let mut alias = p.clone();
        alias[3] = p[1].clone();
        assert!(run(merge(&alias, true), |_| panic!(), &mut Vec::new(), &c).is_err());
        assert_eq!(fs::read(&p[1]).unwrap(), b"left\n");
        fs::remove_file(&p[0]).unwrap();
        assert!(run(merge(&p, false), |_| panic!(), &mut Vec::new(), &c).is_err());
    }
    #[test]
    fn null_roles_remain_distinct_from_existing_empty_files() {
        let (tmp, p) = fixture();
        let null = PathBuf::from(if cfg!(windows) { "nul" } else { "/dev/null" });
        let empty = tmp.path().join("empty");
        fs::write(&empty, []).unwrap();
        for (left, right, missing) in [
            (null.clone(), empty.clone(), (true, false)),
            (empty.clone(), null, (false, true)),
            (empty.clone(), empty.clone(), (false, false)),
        ] {
            let command = GitCommand::Diff {
                local: left,
                remote: right,
                encoding: None,
                headless: false,
            };
            assert_eq!(
                run(
                    command,
                    |g| {
                        assert!(g.git_diff);
                        assert_eq!((g.git_left_missing, g.git_right_missing), missing);
                        assert_eq!(fs::read(g.left.unwrap()).unwrap(), b"");
                        assert_eq!(fs::read(g.right.unwrap()).unwrap(), b"");
                        Ok(0)
                    },
                    &mut Vec::new(),
                    &Cancellation::new()
                )
                .unwrap(),
                0
            );
        }
        assert_eq!(fs::read(&p[3]).unwrap(), b"original output");
    }
    #[test]
    fn diff_waits_and_maps_difference_to_success_but_preserves_errors_and_cancel() {
        let (_tmp, p) = fixture();
        for code in [0, 1, 2, 130] {
            let command = GitCommand::Diff {
                local: p[1].clone(),
                remote: p[2].clone(),
                encoding: None,
                headless: false,
            };
            assert_eq!(
                run(
                    command,
                    |g| {
                        assert!(g.wait);
                        assert_eq!(g.left, Some(p[1].clone()));
                        assert_eq!(g.right, Some(p[2].clone()));
                        Ok(code)
                    },
                    &mut Vec::new(),
                    &Cancellation::new()
                )
                .unwrap(),
                if code == 1 { 0 } else { code }
            );
        }
    }
}
