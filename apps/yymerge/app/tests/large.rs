use std::{fs, io::Write, process::Command};
fn run(args: &[&str], directory: &std::path::Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_yymerge"))
        .current_dir(directory)
        .args(args)
        .output()
        .unwrap()
}
#[test]
fn large_cli_reports_exact_rows_and_preserves_existing_files() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    fs::write(dir.join("left"), b"first\r\nsecond\nend").unwrap();
    fs::write(dir.join("right"), b"first\r\ninsert\nsecond\nend\n").unwrap();
    let args = ["large-diff", "left", "right", "--report", "report.json"];
    let out = run(&args, dir);
    assert_eq!(
        out.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(dir.join("report.json")).unwrap()).unwrap();
    assert_eq!(report["mode"], "disk");
    assert_eq!(report["row_count"], 4);
    assert_eq!(report["changed_rows"], 2);
    assert_eq!(report["rows"][1]["kind"], "RightOnly");
    let saved = fs::read(dir.join("report.json")).unwrap();
    assert_eq!(run(&args, dir).status.code(), Some(2));
    assert_eq!(fs::read(dir.join("report.json")).unwrap(), saved);
    assert_eq!(
        run(&["large-diff", "left", "right", "--report", "left"], dir)
            .status
            .code(),
        Some(2)
    );
    assert_eq!(fs::read(dir.join("left")).unwrap(), b"first\r\nsecond\nend");
}
#[test]
fn default_diff_automatically_compares_files_over_32_mib() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    let mut file = fs::File::create(dir.join("large")).unwrap();
    let mut chunk = vec![b'a'; 64 * 1024];
    *chunk.last_mut().unwrap() = b'\n';
    for _ in 0..530 {
        file.write_all(&chunk).unwrap();
    }
    drop(file);
    let out = run(&["diff", "large", "large"], dir);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("disk index"));
    assert!(String::from_utf8_lossy(&out.stdout).contains("530 rows"));
}
#[test]
fn large_encodings_are_per_side_and_bom_differences_remain_visible() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    fs::write(dir.join("left"), "日本語\n").unwrap();
    fs::write(dir.join("right"), [0x93, 0xfa, 0x96, 0x7b, 0x8c, 0xea, 10]).unwrap();
    assert_eq!(
        run(
            &["large-diff", "left", "right", "--right-encoding", "CP932"],
            dir
        )
        .status
        .code(),
        Some(1)
    );
    assert_eq!(
        run(
            &[
                "large-diff",
                "left",
                "right",
                "--right-encoding",
                "CP932",
                "--ignore-encoding"
            ],
            dir
        )
        .status
        .code(),
        Some(0)
    );
}
