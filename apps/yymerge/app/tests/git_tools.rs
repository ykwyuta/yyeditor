//! Exercise configuration parsing, Git's shell, real index roles and tool exit codes.
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};
fn git(dir: &Path, args: &[&str], input: Option<&[u8]>) -> Output {
    let mut child = Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", dir.join("no-global-config"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    if let Some(bytes) = input {
        child.stdin.take().unwrap().write_all(bytes).unwrap();
    }
    child.wait_with_output().unwrap()
}
fn ok(dir: &Path, args: &[&str], input: Option<&[u8]>) -> Output {
    let out = git(dir, args, input);
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}
fn app(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_yymerge"))
        .current_dir(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", dir.join("no-global-config"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap()
}
fn setup() -> (tempfile::TempDir, PathBuf, String) {
    // Keep the alias on the binary's filesystem so it can be hard-linked.
    // Copying while another test forks can leave an inherited writable fd in
    // that child and make exec fail with ETXTBSY, even after fs::copy returns.
    #[cfg(unix)]
    let tmp =
        tempfile::tempdir_in(Path::new(env!("CARGO_BIN_EXE_yymerge")).parent().unwrap()).unwrap();
    #[cfg(not(unix))]
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().to_path_buf();
    ok(&dir, &["init", "-q", "-b", "master"], None);
    for (name, value) in [
        ("core.autocrlf", "false"),
        ("core.eol", "lf"),
        ("user.name", "yymerge tests"),
        ("user.email", "yymerge@example.invalid"),
    ] {
        ok(&dir, &["config", name, value], None);
    }
    let file = "日本語 file.txt".to_string();
    fs::write(dir.join(&file), b"a\nb\n").unwrap();
    ok(&dir, &["add", "--", &file], None);
    ok(
        &dir,
        &[
            "-c",
            "core.hooksPath=disabled-hooks",
            "commit",
            "-qm",
            "fixture",
        ],
        None,
    );
    // Shell metacharacters and apostrophe in the executable path must remain literal.
    let tools = dir.join("tools 日本語 ' $;`");
    fs::create_dir(&tools).unwrap();
    let exe = tools.join(if cfg!(windows) {
        "yymerge.exe"
    } else {
        "yymerge"
    });
    #[cfg(unix)]
    fs::hard_link(env!("CARGO_BIN_EXE_yymerge"), &exe).unwrap();
    #[cfg(not(unix))]
    fs::copy(env!("CARGO_BIN_EXE_yymerge"), &exe).unwrap();
    let config = Command::new(&exe)
        .args(["git-tool", "config", "--headless"])
        .output()
        .unwrap();
    assert!(config.status.success());
    let config_path = dir.join("yymerge.gitconfig");
    fs::write(&config_path, &config.stdout).unwrap();
    ok(
        &dir,
        &["config", "include.path", config_path.to_str().unwrap()],
        None,
    );
    (tmp, dir, file)
}
fn conflict(dir: &Path, file: &str, contents: [Option<&[u8]>; 3]) {
    ok(dir, &["update-index", "--force-remove", "--", file], None);
    let mut index = Vec::new();
    for (i, content) in contents.into_iter().enumerate() {
        if let Some(bytes) = content {
            let out = ok(dir, &["hash-object", "-w", "--stdin"], Some(bytes));
            let hash = String::from_utf8(out.stdout).unwrap();
            index.extend(format!("100644 {} {}\t{}\0", hash.trim(), i + 1, file).as_bytes());
        }
    }
    ok(dir, &["update-index", "-z", "--index-info"], Some(&index));
    fs::write(dir.join(file), b"existing conflict markers\n").unwrap();
}
#[test]
fn git_mergetool_commits_resolved_output_and_difftool_treats_differences_as_success() {
    let (_tmp, dir, file) = setup();
    conflict(
        &dir,
        &file,
        [Some(b"a\nb\n"), Some(b"A\nb\n"), Some(b"a\nB\n")],
    );
    let result = git(
        &dir,
        &["mergetool", "--no-prompt", "--tool=yymerge", "--", &file],
        None,
    );
    assert_eq!(
        result.status.code(),
        Some(0),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(fs::read(dir.join(&file)).unwrap(), b"A\nB\n");
    assert!(
        ok(&dir, &["ls-files", "--unmerged"], None)
            .stdout
            .is_empty()
    );
    let before = ok(&dir, &["ls-files", "--stage", "-z"], None).stdout;
    let result = git(
        &dir,
        &[
            "difftool",
            "--cached",
            "--no-prompt",
            "--trust-exit-code",
            "--tool=yymerge",
            "--",
            &file,
        ],
        None,
    );
    assert_eq!(
        result.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        ok(&dir, &["ls-files", "--stage", "-z"], None).stdout,
        before
    );
    assert_eq!(fs::read(dir.join(&file)).unwrap(), b"A\nB\n");
    fs::write(dir.join("added.txt"), b"added\n").unwrap();
    ok(&dir, &["add", "--", "added.txt"], None);
    let addition = git(
        &dir,
        &[
            "difftool",
            "--cached",
            "--no-prompt",
            "--trust-exit-code",
            "--tool=yymerge",
            "--",
            "added.txt",
        ],
        None,
    );
    assert_eq!(
        addition.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&addition.stderr)
    );
    fs::remove_file(dir.join(&file)).unwrap();
    ok(&dir, &["add", "-u", "--", &file], None);
    let deletion = git(
        &dir,
        &[
            "difftool",
            "--cached",
            "--no-prompt",
            "--trust-exit-code",
            "--tool=yymerge",
            "--",
            &file,
        ],
        None,
    );
    assert_eq!(
        deletion.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&deletion.stderr)
    );
}
#[test]
fn git_mergetool_conflicts_and_missing_ancestor_do_not_become_success() {
    let (_tmp, dir, file) = setup();
    conflict(
        &dir,
        &file,
        [Some(b"base\n"), Some(b"left\n"), Some(b"right\n")],
    );
    let before = ok(&dir, &["ls-files", "--stage", "-z"], None).stdout;
    let result = git(
        &dir,
        &["mergetool", "--no-prompt", "--tool=yymerge", "--", &file],
        None,
    );
    assert_eq!(
        result.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(String::from_utf8_lossy(&result.stdout).contains("unresolved conflicts"));
    assert_eq!(
        fs::read(dir.join(&file)).unwrap(),
        b"existing conflict markers\n"
    );
    assert_eq!(
        ok(&dir, &["ls-files", "--stage", "-z"], None).stdout,
        before
    );
    // Git may provide an empty temporary BASE for add/add. Index verification
    // must distinguish that from a real, existing ancestor whose content is empty.
    conflict(&dir, &file, [None, Some(b"left\n"), Some(b"right\n")]);
    for (name, bytes) in [
        ("BASE", b"".as_slice()),
        ("LOCAL", b"left\n"),
        ("REMOTE", b"right\n"),
    ] {
        fs::write(dir.join(name), bytes).unwrap();
    }
    let result = app(
        &dir,
        &[
            "git-tool",
            "merge",
            "--base",
            "BASE",
            "--local",
            "LOCAL",
            "--remote",
            "REMOTE",
            "--merged",
            &file,
            "--headless",
            "--verify-index",
        ],
    );
    assert_eq!(result.status.code(), Some(1));
    assert_eq!(
        fs::read(dir.join(&file)).unwrap(),
        b"existing conflict markers\n"
    );
    conflict(&dir, &file, [Some(b""), Some(b"same\n"), Some(b"same\n")]);
    let result = git(
        &dir,
        &["mergetool", "--no-prompt", "--tool=yymerge", "--", &file],
        None,
    );
    assert_eq!(
        result.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(fs::read(dir.join(file)).unwrap(), b"same\n");
}
