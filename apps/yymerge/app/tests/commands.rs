use std::{
    fs,
    path::Path,
    process::{Command, Output},
};
fn run(args: &[&str], dir: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_yymerge"))
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap()
}
#[test]
fn exit_codes_json_unicode_eol_and_usage() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path();
    fs::write(dir.join("日本語.txt"), "猫\r\n").unwrap();
    fs::write(dir.join("right"), "猫\n").unwrap();
    let a = run(
        &["diff", "日本語.txt", "日本語.txt", "--format", "json"],
        dir,
    );
    assert_eq!(a.status.code(), Some(0));
    assert!(
        String::from_utf8(a.stdout)
            .unwrap()
            .contains("\"identical\": true")
    );
    assert_eq!(
        run(&["diff", "日本語.txt", "right"], dir).status.code(),
        Some(1)
    );
    assert_eq!(
        run(&["diff", "missing", "right"], dir).status.code(),
        Some(2)
    );
    assert_eq!(
        run(&["diff", "日本語.txt", "right", "--format", "unified"], dir)
            .status
            .code(),
        Some(2)
    );
    assert_eq!(
        run(&["merge", "--left", "right"], dir).status.code(),
        Some(64)
    );
    assert_eq!(run(&["--help"], dir).status.code(), Some(0));
    assert_eq!(
        run(&["sync", "apply", "foo", "--execute"], dir)
            .status
            .code(),
        Some(64)
    );
}
#[test]
fn merge_and_report_never_overwrite_inputs_or_unresolved_outputs() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path();
    for (name, text) in [("base", "a\nb\n"), ("left", "A\nb\n"), ("right", "a\nB\n")] {
        fs::write(dir.join(name), text).unwrap();
    }
    let args = [
        "merge", "--base", "base", "--left", "left", "--right", "right", "--output", "out",
    ];
    assert_eq!(run(&args, dir).status.code(), Some(0));
    assert_eq!(fs::read_to_string(dir.join("out")).unwrap(), "A\nB\n");
    assert_eq!(run(&args, dir).status.code(), Some(2));
    fs::write(dir.join("right"), "R\nb\n").unwrap();
    assert_eq!(run(&args, dir).status.code(), Some(1));
    assert_eq!(fs::read_to_string(dir.join("out")).unwrap(), "A\nB\n");
    assert_eq!(
        run(
            &["diff", "left", "right", "--report", "left", "--overwrite"],
            dir
        )
        .status
        .code(),
        Some(2)
    );
    assert_eq!(fs::read_to_string(dir.join("left")).unwrap(), "A\nb\n");
}
#[test]
fn folder_comparison_checks_bytes_and_reports_missing_entries() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path();
    fs::create_dir(dir.join("l")).unwrap();
    fs::create_dir(dir.join("r")).unwrap();
    fs::write(dir.join("l/file"), "abc").unwrap();
    fs::write(dir.join("r/file"), "abd").unwrap();
    assert_eq!(
        run(&["folders", "l", "r", "--recursive"], dir)
            .status
            .code(),
        Some(1)
    );
    fs::write(dir.join("r/file"), "abc").unwrap();
    assert_eq!(
        run(&["folders", "l", "r", "--recursive"], dir)
            .status
            .code(),
        Some(0)
    );
    assert_eq!(
        run(&["folders", "l", "r", "--report", "l/report.json"], dir)
            .status
            .code(),
        Some(2)
    );
    fs::write(dir.join("l/extra"), "").unwrap();
    let result = run(
        &["folders", "l", "r", "--recursive", "--format", "json"],
        dir,
    );
    assert_eq!(result.status.code(), Some(1));
    assert!(
        String::from_utf8(result.stdout)
            .unwrap()
            .contains("LeftOnly")
    );
}
#[cfg(unix)]
#[test]
fn folder_links_are_never_followed_and_return_incomplete() {
    use std::os::unix::fs::symlink;
    let d = tempfile::tempdir().unwrap();
    let dir = d.path();
    fs::create_dir(dir.join("l")).unwrap();
    fs::create_dir(dir.join("r")).unwrap();
    symlink("..", dir.join("l/cycle")).unwrap();
    let result = run(&["folders", "l", "r", "--recursive"], dir);
    assert_eq!(result.status.code(), Some(2));
    assert!(
        String::from_utf8(result.stderr)
            .unwrap()
            .contains("incomplete")
    );
}
#[test]
fn unified_patch_applies_to_exact_bytes_with_git() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path();
    let a = "日本\na\n末尾";
    let b = "日本\nB\n追加\n末尾";
    fs::write(dir.join("left"), a).unwrap();
    fs::write(dir.join("right"), b).unwrap();
    let out = run(&["diff", "left", "right", "--format", "unified"], dir);
    assert_eq!(out.status.code(), Some(1));
    fs::write(dir.join("change.patch"), out.stdout).unwrap();
    // git apply chooses the right-hand name; place the original under that name.
    fs::write(dir.join("right"), a).unwrap();
    let status = Command::new("git")
        .args(["apply", "--no-index", "change.patch"])
        .current_dir(dir)
        .status()
        .expect("git required for integration test");
    assert!(status.success());
    assert_eq!(fs::read_to_string(dir.join("right")).unwrap(), b);
}
