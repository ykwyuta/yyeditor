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
fn html_reports_share_rule_status_escape_source_and_protect_all_inputs() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path();
    fs::write(
        path.join("left"),
        "<svg onload=\"alert('x')\">\r\nALPHA \t\r\n",
    )
    .unwrap();
    fs::write(path.join("right"), "<svg onload=\"alert('x')\">\nalpha\n").unwrap();
    let strict = run(&["diff", "left", "right", "--format", "html"], path);
    assert_eq!(strict.status.code(), Some(1));
    let text = String::from_utf8(strict.stdout).unwrap();
    assert!(text.contains("data-status=\"Different\""));
    assert!(text.contains("&lt;svg onload=&quot;alert(&#39;x&#39;)&quot;&gt;"));
    assert!(!text.contains("<svg"));
    assert!(text.contains("data-eol=\"CRLF\""));
    assert!(text.contains("data-eol=\"LF\""));
    assert_eq!(
        run(
            &[
                "rules-profile",
                "save",
                "rules.json",
                "--ignore-case",
                "--ignore-trailing-space",
                "--ignore-eol"
            ],
            path
        )
        .status
        .code(),
        Some(0)
    );
    let args = [
        "diff",
        "left",
        "right",
        "--rules-profile",
        "rules.json",
        "--format",
        "html",
        "--report",
        "review.html",
    ];
    assert_eq!(run(&args, path).status.code(), Some(0));
    let report = fs::read_to_string(path.join("review.html")).unwrap();
    assert!(report.contains("EquivalentUnderRules"));
    assert!(report.contains("<tr class=ignored>"));
    assert!(report.contains("ALPHA \t"));
    assert_eq!(run(&args, path).status.code(), Some(2));
    assert_eq!(
        fs::read_to_string(path.join("review.html")).unwrap(),
        report
    );
    for target in ["left", "right", "rules.json"] {
        let before = fs::read(path.join(target)).unwrap();
        assert_eq!(
            run(
                &[
                    "diff",
                    "left",
                    "right",
                    "--rules-profile",
                    "rules.json",
                    "--format",
                    "html",
                    "--report",
                    target,
                    "--overwrite"
                ],
                path
            )
            .status
            .code(),
            Some(2)
        );
        assert_eq!(fs::read(path.join(target)).unwrap(), before);
    }
    fs::write(
        path.join("right"),
        "\u{feff}<svg onload=\"alert('x')\">\nalpha\n",
    )
    .unwrap();
    let out = run(
        &[
            "diff",
            "left",
            "right",
            "--ignore-case",
            "--ignore-trailing-space",
            "--ignore-eol",
            "--format",
            "html",
        ],
        path,
    );
    assert_eq!(out.status.code(), Some(1));
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("data-status=\"Different\""));
    assert!(text.contains("BOM: [239, 187, 191]"));
}
#[test]
fn html_output_expansion_refuses_partial_stdout_and_preserves_existing_report() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path();
    fs::write(path.join("large"), "<".repeat(9 * 1024 * 1024)).unwrap();
    fs::write(path.join("review.html"), "keep").unwrap();
    for args in [
        vec!["diff", "large", "large", "--format", "html"],
        vec![
            "diff",
            "large",
            "large",
            "--format",
            "html",
            "--report",
            "review.html",
            "--overwrite",
        ],
    ] {
        let out = run(&args, path);
        assert_eq!(out.status.code(), Some(2));
        assert!(out.stdout.is_empty());
        assert!(
            String::from_utf8(out.stderr)
                .unwrap()
                .contains("HTML report incomplete")
        );
    }
    assert_eq!(
        fs::read_to_string(path.join("review.html")).unwrap(),
        "keep"
    );
}
#[test]
fn profiles_round_trip_cli_rules_and_preserve_inputs_and_existing_reports() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path();
    fs::write(dir.join("left"), "ALPHA \t\r\n").unwrap();
    fs::write(dir.join("right"), "alpha\n").unwrap();
    let profile = "比較 規則.json";
    let save = [
        "rules-profile",
        "save",
        profile,
        "--ignore-case",
        "--ignore-trailing-space",
        "--ignore-eol",
    ];
    assert_eq!(run(&save, dir).status.code(), Some(0));
    let bytes = fs::read(dir.join(profile)).unwrap();
    assert_eq!(run(&save, dir).status.code(), Some(2));
    assert_eq!(fs::read(dir.join(profile)).unwrap(), bytes);
    let show = run(&["rules-profile", "show", profile], dir);
    assert_eq!(show.status.code(), Some(0));
    assert_eq!(show.stdout, bytes);
    let out = run(
        &[
            "diff",
            "left",
            "right",
            "--rules-profile",
            profile,
            "--format",
            "json",
        ],
        dir,
    );
    assert_eq!(out.status.code(), Some(0));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["status"], "EquivalentUnderRules");
    assert_eq!(json["identical"], false);
    assert_eq!(json["ignored_line_pairs"].as_array().unwrap().len(), 1);
    assert_eq!(run(&["diff", "left", "right"], dir).status.code(), Some(1));
    assert_eq!(
        run(
            &[
                "diff",
                "left",
                "right",
                "--rules-profile",
                profile,
                "--ignore-case"
            ],
            dir
        )
        .status
        .code(),
        Some(64)
    );
    assert_eq!(
        run(
            &[
                "diff",
                "left",
                "right",
                "--rules-profile",
                profile,
                "--report",
                profile,
                "--overwrite"
            ],
            dir
        )
        .status
        .code(),
        Some(2)
    );
    assert_eq!(
        run(
            &[
                "rules-profile",
                "save",
                profile,
                "--rules-profile",
                profile,
                "--overwrite"
            ],
            dir
        )
        .status
        .code(),
        Some(2)
    );
    assert_eq!(fs::read(dir.join(profile)).unwrap(), bytes);
    assert_eq!(fs::read(dir.join("left")).unwrap(), b"ALPHA \t\r\n");
    fs::write(dir.join("report"), "keep").unwrap();
    assert_eq!(
        run(
            &[
                "diff",
                "left",
                "right",
                "--rules-profile",
                profile,
                "--format",
                "unified",
                "--report",
                "report",
                "--overwrite"
            ],
            dir
        )
        .status
        .code(),
        Some(2)
    );
    assert_eq!(fs::read_to_string(dir.join("report")).unwrap(), "keep");
    assert_eq!(
        run(&["rules-profile", "save", profile, "--overwrite"], dir)
            .status
            .code(),
        Some(0)
    );
    assert_eq!(
        run(&["diff", "left", "right", "--rules-profile", profile], dir)
            .status
            .code(),
        Some(1)
    );
}
#[test]
fn invalid_profiles_never_fall_back_to_strict_or_replace_reports() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path();
    fs::write(dir.join("left"), "same\n").unwrap();
    fs::write(dir.join("report"), "keep").unwrap();
    for invalid in ["{}", "{\"version\":2,\"rules\":{}}", "not JSON"] {
        fs::write(dir.join("profile"), invalid).unwrap();
        for args in [
            vec!["rules-profile", "show", "profile"],
            vec![
                "diff",
                "left",
                "left",
                "--rules-profile",
                "profile",
                "--report",
                "report",
                "--overwrite",
            ],
        ] {
            assert_eq!(run(&args, dir).status.code(), Some(2));
        }
        assert_eq!(fs::read_to_string(dir.join("report")).unwrap(), "keep");
    }
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
fn three_way_folders_report_deletion_conflicts_and_protect_the_ancestor() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path();
    for root in ["base", "left", "right"] {
        fs::create_dir(dir.join(root)).unwrap();
    }
    for (path, value) in [
        ("base/conflict", "original"),
        ("right/conflict", "changed"),
        ("base/empty", ""),
        ("left/empty", ""),
        ("right/empty", ""),
        ("left/new", "same"),
        ("right/new", "same"),
    ] {
        fs::write(dir.join(path), value).unwrap();
    }
    let result = run(
        &[
            "folders",
            "left",
            "right",
            "--base",
            "base",
            "--recursive",
            "--format",
            "json",
        ],
        dir,
    );
    assert_eq!(result.status.code(), Some(1));
    let rows: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    let row = |name| {
        rows.as_array()
            .unwrap()
            .iter()
            .find(|e| e["path"] == name)
            .unwrap()
    };
    assert_eq!(row("conflict")["status"], "DeleteModifyConflict");
    assert!(row("conflict")["left"].is_null());
    assert_eq!(row("empty")["status"], "Unchanged");
    assert_eq!(row("new")["status"], "SameAddition");
    assert_eq!(
        run(
            &[
                "folders",
                "left",
                "right",
                "--base",
                "base",
                "--report",
                "base/report"
            ],
            dir
        )
        .status
        .code(),
        Some(2)
    );
    assert!(!dir.join("base/report").exists());
    fs::remove_file(dir.join("right/conflict")).unwrap();
    fs::write(dir.join("left/conflict"), "original").unwrap();
    fs::write(dir.join("right/conflict"), "original").unwrap();
    for root in ["left", "right"] {
        fs::remove_file(dir.join(root).join("new")).unwrap();
    }
    assert_eq!(
        run(&["folders", "left", "right", "--base", "base"], dir)
            .status
            .code(),
        Some(0)
    );
    assert_eq!(
        run(&["folders", "left", "right", "--base", "missing"], dir)
            .status
            .code(),
        Some(2)
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
    for autocrlf in ["true", "false", "input"] {
        // Simulate host settings without modifying the user's Git configuration.
        // Exact-byte patch verification must explicitly disable Git's EOL conversion.
        // git apply chooses the right-hand name; put the original under that name.
        fs::write(dir.join("right"), a).unwrap();
        let status = Command::new("git")
            .env("GIT_CONFIG_COUNT", "2")
            .env("GIT_CONFIG_KEY_0", "core.autocrlf")
            .env("GIT_CONFIG_VALUE_0", autocrlf)
            .env("GIT_CONFIG_KEY_1", "core.eol")
            .env("GIT_CONFIG_VALUE_1", "crlf")
            .args([
                "-c",
                "core.autocrlf=false",
                "-c",
                "core.eol=lf",
                "apply",
                "--no-index",
                "change.patch",
            ])
            .current_dir(dir)
            .status()
            .expect("git required for integration test");
        assert!(status.success(), "core.autocrlf={autocrlf}");
        assert_eq!(
            fs::read(dir.join("right")).unwrap(),
            b.as_bytes(),
            "core.autocrlf={autocrlf}"
        );
    }
}

#[test]
fn sync_plans_are_preview_only_guard_reports_and_never_execute_changes() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path();
    for root in ["base", "left", "right"] {
        fs::create_dir(dir.join(root)).unwrap();
    }
    for (path, data) in [
        ("left/new", "left"),
        ("right/orphan", "right"),
        ("base/both", "base"),
        ("left/both", "left"),
        ("right/both", "right"),
    ] {
        fs::write(dir.join(path), data).unwrap();
    }
    let result = run(
        &[
            "sync",
            "plan",
            "--left",
            "left",
            "--right",
            "right",
            "--mode",
            "mirror-right",
        ],
        dir,
    );
    assert_eq!(result.status.code(), Some(0));
    let plan: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(plan["preview_only"], true);
    assert!(plan["summary"]["operations"].as_u64().unwrap() > 0);
    let orphan = plan["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["path"] == "orphan")
        .unwrap();
    assert_eq!(orphan["action"], "DeleteFile");
    assert!(orphan["comparison"].is_null());
    assert_eq!(
        fs::read_to_string(dir.join("right/orphan")).unwrap(),
        "right"
    );
    assert_eq!(fs::read_to_string(dir.join("right/both")).unwrap(), "right");
    assert!(!dir.join("right/new").exists());
    let result = run(
        &[
            "sync", "plan", "--left", "left", "--right", "right", "--base", "base", "--mode",
            "merge",
        ],
        dir,
    );
    assert_eq!(result.status.code(), Some(1));
    let plan: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert!(
        plan["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["action"] == "NeedsMerge")
    );
    assert_eq!(
        run(
            &[
                "sync",
                "plan",
                "--left",
                "left",
                "--right",
                "right",
                "--base",
                "base",
                "--mode",
                "merge",
                "--output",
                "base/plan.json"
            ],
            dir
        )
        .status
        .code(),
        Some(2)
    );
    assert!(!dir.join("base/plan.json").exists());
    fs::write(dir.join("plan.json"), "existing").unwrap();
    assert_eq!(
        run(
            &[
                "sync",
                "plan",
                "--left",
                "left",
                "--right",
                "right",
                "--output",
                "plan.json"
            ],
            dir
        )
        .status
        .code(),
        Some(2)
    );
    assert_eq!(
        fs::read_to_string(dir.join("plan.json")).unwrap(),
        "existing"
    );
    assert_eq!(
        run(
            &[
                "sync",
                "plan",
                "--left",
                "left",
                "--right",
                "right",
                "--output",
                "plan.json",
                "--overwrite"
            ],
            dir
        )
        .status
        .code(),
        Some(0)
    );
    assert_eq!(
        run(
            &[
                "sync", "plan", "--left", "left", "--right", "right", "--mode", "merge"
            ],
            dir
        )
        .status
        .code(),
        Some(64)
    );
    assert_eq!(
        run(&["sync", "apply", "plan.json", "--execute"], dir)
            .status
            .code(),
        Some(64)
    );
}

#[test]
fn sync_apply_requires_opt_in_rejects_stale_plans_and_restores_originals() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path();
    fs::create_dir(dir.join("left")).unwrap();
    fs::create_dir(dir.join("right")).unwrap();
    fs::write(dir.join("left/new"), b"new").unwrap();
    fs::write(dir.join("left/file"), b"updated").unwrap();
    fs::write(dir.join("right/file"), b"original").unwrap();
    fs::write(dir.join("right/old"), b"old").unwrap();
    let make = || {
        run(
            &[
                "sync",
                "plan",
                "--left",
                "left",
                "--right",
                "right",
                "--mode",
                "mirror-right",
                "--output",
                "plan.json",
                "--overwrite",
            ],
            dir,
        )
    };
    assert_eq!(make().status.code(), Some(0));
    assert_eq!(
        run(&["sync", "apply", "plan.json"], dir).status.code(),
        Some(0)
    );
    assert_eq!(fs::read(dir.join("right/file")).unwrap(), b"original");
    for args in [
        vec!["sync", "apply", "plan.json", "--execute"],
        vec!["sync", "apply", "plan.json", "--run-dir", "run"],
        vec!["sync", "rollback", "run"],
    ] {
        assert_eq!(run(&args, dir).status.code(), Some(64));
    }
    fs::write(dir.join("left/new"), b"new edit").unwrap();
    assert_eq!(
        run(
            &[
                "sync",
                "apply",
                "plan.json",
                "--execute",
                "--run-dir",
                "run"
            ],
            dir
        )
        .status
        .code(),
        Some(2)
    );
    assert!(!dir.join("run").exists());
    assert_eq!(make().status.code(), Some(0));
    let applied = run(
        &[
            "sync",
            "apply",
            "plan.json",
            "--execute",
            "--run-dir",
            "run",
        ],
        dir,
    );
    assert_eq!(
        applied.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&applied.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&applied.stdout).unwrap();
    assert!(report["error"].is_null());
    assert_eq!(fs::read(dir.join("right/file")).unwrap(), b"updated");
    assert!(!dir.join("right/old").exists());
    assert!(dir.join("run/journal.jsonl").exists());
    let restored = run(&["sync", "rollback", "run", "--execute"], dir);
    assert_eq!(
        restored.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&restored.stderr)
    );
    assert_eq!(fs::read(dir.join("right/file")).unwrap(), b"original");
    assert_eq!(fs::read(dir.join("right/old")).unwrap(), b"old");
    assert!(!dir.join("right/new").exists());
    assert_eq!(
        run(&["sync", "rollback", "run", "--execute"], dir)
            .status
            .code(),
        Some(0)
    );
}

#[test]
fn sync_resume_continues_pending_copy_and_refuses_after_rollback() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    fs::create_dir(dir.join("left")).unwrap();
    fs::create_dir(dir.join("right")).unwrap();
    fs::write(dir.join("left/file"), b"new").unwrap();
    fs::write(dir.join("right/file"), b"original").unwrap();
    assert_eq!(
        run(
            &[
                "sync",
                "plan",
                "--left",
                "left",
                "--right",
                "right",
                "--output",
                "plan.json"
            ],
            dir
        )
        .status
        .code(),
        Some(0)
    );
    assert_eq!(
        run(
            &[
                "sync",
                "apply",
                "plan.json",
                "--execute",
                "--run-dir",
                "run"
            ],
            dir
        )
        .status
        .code(),
        Some(0)
    );
    let path = dir.join("run/journal.jsonl");
    let log = fs::read_to_string(&path).unwrap();
    fs::write(&path, log.strip_suffix("\"Finished\"\n").unwrap()).unwrap();
    fs::rename(dir.join("right/file"), dir.join("run/staging/0")).unwrap();
    assert_eq!(run(&["sync", "resume", "run"], dir).status.code(), Some(64));
    assert!(!dir.join("right/file").exists());
    let resumed = run(&["sync", "resume", "run", "--execute"], dir);
    assert_eq!(
        resumed.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&resumed.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&resumed.stdout).unwrap();
    assert_eq!(report["completed"], 1);
    assert_eq!(fs::read(dir.join("right/file")).unwrap(), b"new");
    assert_eq!(fs::read(dir.join("run/backups/0")).unwrap(), b"original");
    assert_eq!(
        run(&["sync", "resume", "run", "--execute"], dir)
            .status
            .code(),
        Some(0)
    );
    assert_eq!(
        run(&["sync", "rollback", "run", "--execute"], dir)
            .status
            .code(),
        Some(0)
    );
    assert_eq!(
        run(&["sync", "resume", "run", "--execute"], dir)
            .status
            .code(),
        Some(2)
    );
    assert_eq!(fs::read(dir.join("right/file")).unwrap(), b"original");
}

#[test]
fn comparison_rules_report_equivalence_without_claiming_original_identity() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    fs::write(dir.join("left"), b"Alpha \t\r\nkeep\r\n").unwrap();
    fs::write(dir.join("right"), b"alpha\nkeep\n").unwrap();
    assert_eq!(run(&["diff", "left", "right"], dir).status.code(), Some(1));
    let result = run(
        &[
            "diff",
            "left",
            "right",
            "--ignore-case",
            "--ignore-trailing-space",
            "--ignore-eol",
            "--format",
            "json",
        ],
        dir,
    );
    assert_eq!(result.status.code(), Some(0));
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["identical"], false);
    assert_eq!(report["equivalent"], true);
    assert_eq!(report["status"], "EquivalentUnderRules");
    assert_eq!(report["ignored_line_pairs"].as_array().unwrap().len(), 2);
    assert_eq!(report["ignored_line_pairs"][1]["left"], 1);
    assert_eq!(report["rules"]["ignore_case"], true);
    fs::write(dir.join("right"), b"alpha\nchanged\n").unwrap();
    let result = run(
        &[
            "diff",
            "left",
            "right",
            "--ignore-case",
            "--ignore-trailing-space",
            "--ignore-eol",
            "--format",
            "json",
        ],
        dir,
    );
    assert_eq!(result.status.code(), Some(1));
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["status"], "Different");
    assert_eq!(report["ignored_line_pairs"].as_array().unwrap().len(), 1);
    assert_eq!(fs::read(dir.join("left")).unwrap(), b"Alpha \t\r\nkeep\r\n");
}
#[test]
fn display_rules_never_generate_normalized_patches_or_change_merge_and_sync_semantics() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    for (name, bytes) in [
        ("base", b"base\n".as_slice()),
        ("left", b"A\n"),
        ("right", b"a\n"),
        ("result", b"keep me"),
        ("patch", b"keep patch"),
    ] {
        fs::write(dir.join(name), bytes).unwrap();
    }
    assert_eq!(
        run(&["diff", "left", "right", "--ignore-case"], dir)
            .status
            .code(),
        Some(0)
    );
    assert_eq!(
        run(
            &[
                "diff",
                "left",
                "right",
                "--ignore-case",
                "--format",
                "unified",
                "--report",
                "patch",
                "--overwrite"
            ],
            dir
        )
        .status
        .code(),
        Some(2)
    );
    assert_eq!(fs::read(dir.join("patch")).unwrap(), b"keep patch");
    assert_eq!(
        run(
            &[
                "merge", "--base", "base", "--left", "left", "--right", "right", "--output",
                "result"
            ],
            dir
        )
        .status
        .code(),
        Some(1)
    );
    assert_eq!(fs::read(dir.join("result")).unwrap(), b"keep me");
    assert_eq!(
        run(
            &[
                "merge",
                "--base",
                "base",
                "--left",
                "left",
                "--right",
                "right",
                "--output",
                "result",
                "--ignore-case"
            ],
            dir
        )
        .status
        .code(),
        Some(64)
    );
    let gui = run(
        &[
            "gui",
            "--base",
            "base",
            "--left",
            "left",
            "--right",
            "right",
            "--output",
            "result",
            "--ignore-case",
            "--wait",
        ],
        dir,
    );
    assert_eq!(gui.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&gui.stderr).contains("two-way"));
    for root in ["l-dir", "r-dir"] {
        fs::create_dir(dir.join(root)).unwrap();
    }
    fs::write(dir.join("l-dir/file"), b"A\n").unwrap();
    fs::write(dir.join("r-dir/file"), b"a\n").unwrap();
    let plan = run(
        &["sync", "plan", "--left", "l-dir", "--right", "r-dir"],
        dir,
    );
    assert_eq!(plan.status.code(), Some(0));
    let report: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    assert_eq!(report["items"][0]["action"], "ReplaceFile");
    assert_eq!(fs::read(dir.join("r-dir/file")).unwrap(), b"a\n");
}
#[test]
fn rules_keep_bom_encoding_and_final_newline_differences_visible() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    fs::write(dir.join("left"), b"\xef\xbb\xbfA\n").unwrap();
    fs::write(dir.join("right"), b"a\n").unwrap();
    let result = run(
        &["diff", "left", "right", "--ignore-case", "--format", "json"],
        dir,
    );
    assert_eq!(result.status.code(), Some(1));
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["equivalent"], false);
    assert_ne!(report["left_bom"], report["right_bom"]);
    fs::write(dir.join("left"), b"A\r\n").unwrap();
    fs::write(dir.join("right"), b"a").unwrap();
    assert_eq!(
        run(
            &["diff", "left", "right", "--ignore-case", "--ignore-eol"],
            dir
        )
        .status
        .code(),
        Some(1)
    );
}
