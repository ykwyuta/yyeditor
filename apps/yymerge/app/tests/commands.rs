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
fn per_file_encoding_byte_review_and_session_cli() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    fs::write(p.join("left"), "日本語\n".as_bytes()).unwrap();
    fs::write(p.join("right"), [0x93, 0xfa, 0x96, 0x7b, 0x8c, 0xea, 0x0a]).unwrap();
    let args = [
        "diff",
        "left",
        "right",
        "--left-encoding",
        "UTF-8",
        "--right-encoding",
        "CP932",
        "--format",
        "json",
    ];
    let out = run(&args, p);
    assert_eq!(
        out.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let mut ignore = args.to_vec();
    ignore.push("--ignore-encoding");
    let out = run(&ignore, p);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["identical"], false);
    assert_eq!(v["equivalent"], true);
    assert_eq!(v["left_encoding"], "UTF-8");
    assert_eq!(v["right_encoding"], "CP932");
    assert_eq!(
        run(
            &[
                "comparison-session",
                "create",
                "--left",
                "left",
                "--right",
                "right",
                "--left-encoding",
                "UTF-8",
                "--right-encoding",
                "CP932",
                "--ignore-encoding",
                "--output",
                "state.json"
            ],
            p
        )
        .status
        .code(),
        Some(0)
    );
    let out = run(&["comparison-session", "inspect", "state.json"], p);
    assert_eq!(out.status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["version"], 3);
    assert_eq!(v["equivalent"], true);
    // Packed decimal and invalid text must not fail, nor collapse distinct bytes.
    fs::write(p.join("left"), [0x12, 0x3c, 0, 0xff]).unwrap();
    fs::write(p.join("right"), [0x12, 0x3c, 0, 0xfe]).unwrap();
    assert_eq!(run(&["diff", "left", "right"], p).status.code(), Some(2));
    let out = run(
        &[
            "diff",
            "left",
            "right",
            "--ignore-encoding",
            "--format",
            "html",
        ],
        p,
    );
    assert_eq!(
        out.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let html = String::from_utf8(out.stdout).unwrap();
    assert!(html.contains("\\xFF") && html.contains("\\xFE"));
    assert_eq!(
        run(&["diff", "left", "left", "--ignore-encoding"], p)
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
                "--ignore-encoding",
                "--format",
                "unified"
            ],
            p
        )
        .status
        .code(),
        Some(2)
    );
    assert_eq!(
        run(
            &[
                "diff",
                "left",
                "right",
                "--encoding",
                "UTF-8",
                "--left-encoding",
                "CP932"
            ],
            p
        )
        .status
        .code(),
        Some(64)
    );
    // Same raw bytes can have different selected interpretations.
    fs::write(p.join("left"), b"AB").unwrap();
    let out = run(
        &[
            "diff",
            "left",
            "left",
            "--left-encoding",
            "UTF-8",
            "--right-encoding",
            "UTF-16LE",
            "--ignore-encoding",
            "--format",
            "json",
        ],
        p,
    );
    assert_eq!(out.status.code(), Some(1));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["status"], "Different");
    // BOM differences require the explicit ignore flag.
    fs::write(p.join("right"), b"\xef\xbb\xbfAB").unwrap();
    assert_eq!(
        run(
            &[
                "diff",
                "left",
                "right",
                "--left-encoding",
                "UTF-8",
                "--right-encoding",
                "UTF-8"
            ],
            p
        )
        .status
        .code(),
        Some(1)
    );
    assert_eq!(
        run(&["diff", "left", "right", "--ignore-encoding"], p)
            .status
            .code(),
        Some(0)
    );
}
#[test]
fn image_cli_pixel_threshold_alignment_and_reports_share_the_gui_model() {
    use yy_merge_core::{
        Cancellation,
        images::{self, Raster},
    };
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    let c = Cancellation::new();
    let a = Raster::new(2, 1, vec![100, 120, 130, 255, 10, 20, 30, 255]).unwrap();
    let b = Raster::new(2, 1, vec![105, 120, 130, 255, 10, 20, 30, 255]).unwrap();
    fs::write(p.join("a.png"), images::png(&a, &c).unwrap()).unwrap();
    fs::write(p.join("b.png"), images::png(&b, &c).unwrap()).unwrap();
    let args = [
        "image-diff",
        "a.png",
        "b.png",
        "--diff-image",
        "difference.png",
        "--report",
        "report.json",
    ];
    let out = run(&args, p);
    assert_eq!(
        out.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value =
        serde_json::from_slice(&fs::read(p.join("report.json")).unwrap()).unwrap();
    assert_eq!(v["comparison"]["changed_pixels"], 1);
    assert_eq!(v["original_identical"], false);
    assert!(images::Input::open(&p.join("difference.png"), &c).is_ok());
    let out = run(&["image-diff", "a.png", "b.png", "--threshold", "5"], p);
    assert_eq!(out.status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["comparison"]["raw_difference_pixels"], 1);
    assert_eq!(v["comparison"]["changed_pixels"], 0);
    let out = run(&["image-diff", "a.png", "b.png", "--offset-x", "-1"], p);
    assert_eq!(out.status.code(), Some(1));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["comparison"]["canvas"]["x"], -1);
    for format in ["json", "text", "html"] {
        let out = run(&["diff", "a.png", "b.png", "--format", format], p);
        assert_eq!(out.status.code(), Some(1));
        assert!(!out.stdout.is_empty());
        if format == "html" {
            assert!(String::from_utf8_lossy(&out.stdout).contains("data:image/png;base64,"));
        }
    }
    assert_eq!(
        run(&["image-diff", "a.png", "a.png"], p).status.code(),
        Some(0)
    );
}
#[test]
fn webp_cli_auto_detection_and_cross_format_reports() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    for (name, bytes) in [
        (
            "left.webp",
            include_bytes!("../../../../demo/yymerge/images/left.webp").as_slice(),
        ),
        (
            "left.png",
            include_bytes!("../../../../demo/yymerge/images/left.png").as_slice(),
        ),
        (
            "right.webp",
            include_bytes!("../../../../demo/yymerge/images/right.webp").as_slice(),
        ),
        (
            "animated.webp",
            include_bytes!("../../../../demo/yymerge/images/animated.webp").as_slice(),
        ),
    ] {
        fs::write(p.join(name), bytes).unwrap();
    }
    let out = run(&["diff", "left.webp", "left.png", "--format", "json"], p);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["mode"], "image");
    assert_eq!(v["original_identical"], false);
    let out = run(
        &[
            "image-diff",
            "left.webp",
            "right.webp",
            "--format",
            "html",
            "--diff-image",
            "difference.png",
        ],
        p,
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stdout).contains("data:image/png;base64,"));
    assert!(
        yy_merge_core::images::Input::open(
            &p.join("difference.png"),
            &yy_merge_core::Cancellation::new()
        )
        .is_ok()
    );
    let out = run(
        &["diff", "animated.webp", "left.webp", "--format", "json"],
        p,
    );
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
    assert!(String::from_utf8_lossy(&out.stderr).contains("animated WebP"));
}
#[test]
fn pdf_cli_compares_pages_without_external_tools_and_preserves_sources() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    let left = include_bytes!("../../../../demo/yymerge/pdf/left.pdf");
    fs::write(p.join("left.pdf"), left).unwrap();
    fs::write(
        p.join("right.PDF"),
        include_bytes!("../../../../demo/yymerge/pdf/right.pdf"),
    )
    .unwrap();
    fs::write(
        p.join("password.pdf"),
        include_bytes!("../../../../demo/yymerge/pdf/password.pdf"),
    )
    .unwrap();
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_yymerge"))
            .current_dir(p)
            .args(args)
            .env("YYMERGE_SOFFICE", p.join("must-not-run"))
            .env("YYMERGE_PDFINFO", p.join("must-not-run"))
            .env("YYMERGE_PDFTOPPM", p.join("must-not-run"))
            .output()
            .unwrap()
    };
    let out = run(&["diff", "left.pdf", "right.PDF", "--format", "json"]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["pdf_rendered"], true);
    assert_eq!(json["office_rendered"], false);
    assert_eq!(json["pages"], 3);
    assert_eq!(json["equivalent"], false);
    assert_eq!(json["page_results"].as_array().unwrap().len(), 3);
    fs::write(
        p.join("later.pdf"),
        include_bytes!("../../../../demo/yymerge/pdf/later.pdf"),
    )
    .unwrap();
    assert_eq!(
        run(&["image-diff", "left.pdf", "later.pdf", "--page", "1"])
            .status
            .code(),
        Some(0)
    );
    assert_eq!(
        run(&["diff", "left.pdf", "later.pdf"]).status.code(),
        Some(1)
    );
    let out = run(&["pdf-diff", "left.pdf", "later.pdf", "--format", "html"]);
    assert_eq!(out.status.code(), Some(1));
    let html = String::from_utf8(out.stdout).unwrap();
    assert_eq!(html.matches("<article id=page-").count(), 2);
    assert_eq!(html.matches("</html>").count(), 1);
    let out = run(&[
        "image-diff",
        "left.pdf",
        "right.PDF",
        "--page",
        "3",
        "--threshold",
        "255",
        "--diff-image",
        "difference.png",
    ]);
    assert_eq!(out.status.code(), Some(1));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["page"], 3);
    assert_eq!(json["missing"], serde_json::json!([true, false]));
    let image = yy_merge_core::images::Input::open(
        &p.join("difference.png"),
        &yy_merge_core::Cancellation::new(),
    )
    .unwrap();
    assert_eq!(&image.raster.rgba[..4], &[255, 144, 0, 255]);
    let out = run(&[
        "image-diff",
        "left.pdf",
        "left.pdf",
        "--page",
        "2",
        "--format",
        "html",
    ]);
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("PDF rendered page 2 / 2"));
    for args in [
        vec!["image-diff", "left.pdf", "right.PDF", "--page", "4"],
        vec!["diff", "left.pdf", "right.PDF", "--format", "unified"],
        vec!["diff", "password.pdf", "left.pdf"],
        vec![
            "pdf-diff",
            "left.pdf",
            "left.pdf",
            "--report",
            "left.pdf",
            "--overwrite",
        ],
        vec![
            "merge",
            "--base",
            "left.pdf",
            "--left",
            "left.pdf",
            "--right",
            "right.PDF",
            "--output",
            "result.pdf",
        ],
        vec![
            "image-diff",
            "left.pdf",
            "left.pdf",
            "--report",
            "left.pdf",
            "--overwrite",
        ],
    ] {
        let out = run(&args);
        assert_eq!(
            out.status.code(),
            Some(2),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(out.stdout.is_empty());
    }
    assert_eq!(fs::read(p.join("left.pdf")).unwrap(), left);
    assert!(!p.join("result.pdf").exists());
}
#[test]
fn office_render_cli_validates_page_and_document_kind_before_launch() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    fs::write(
        p.join("a.xlsx"),
        include_bytes!("../../../../demo/yymerge/office/left.xlsx"),
    )
    .unwrap();
    fs::write(
        p.join("b.pptx"),
        include_bytes!("../../../../demo/yymerge/office/left.pptx"),
    )
    .unwrap();
    assert_eq!(
        run(&["image-diff", "a.xlsx", "b.pptx"], p).status.code(),
        Some(2)
    );
    assert_eq!(
        run(&["image-diff", "a.xlsx", "a.xlsx", "--page", "0"], p)
            .status
            .code(),
        Some(64)
    );
    assert_eq!(
        run(&["image-diff", "a.xlsx", "a.xlsx", "--page", "129"], p)
            .status
            .code(),
        Some(64)
    );
}
#[test]
#[ignore = "requires LibreOffice; explicitly run in renderer CI"]
fn office_render_cli_real_html_and_source_protection() {
    // Obsolete Poppler settings must not be consulted by the embedded renderer.
    let run = |args: &[&str], dir: &Path| {
        Command::new(env!("CARGO_BIN_EXE_yymerge"))
            .current_dir(dir)
            .args(args)
            .env("YYMERGE_PDFINFO", dir.join("poppler-must-not-run"))
            .env("YYMERGE_PDFTOPPM", dir.join("poppler-must-not-run"))
            .output()
            .unwrap()
    };
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    let left = include_bytes!("../../../../demo/yymerge/office/left.pptx");
    fs::write(p.join("a.pptx"), left).unwrap();
    fs::write(
        p.join("b.pptx"),
        include_bytes!("../../../../demo/yymerge/office/right.pptx"),
    )
    .unwrap();
    let out = run(
        &[
            "image-diff",
            "a.pptx",
            "b.pptx",
            "--page",
            "2",
            "--diff-image",
            "difference.png",
        ],
        p,
    );
    assert_eq!(
        out.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["page"], 2);
    assert_eq!(v["pages"], 2);
    assert_eq!(v["office_rendered"], true);
    assert!(
        yy_merge_core::images::Input::open(
            &p.join("difference.png"),
            &yy_merge_core::Cancellation::new()
        )
        .is_ok()
    );
    let out = run(&["image-diff", "a.pptx", "a.pptx", "--format", "html"], p);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("Office rendered page 1 / 2"));
    let out = run(
        &[
            "image-diff",
            "a.pptx",
            "a.pptx",
            "--report",
            "a.pptx",
            "--overwrite",
        ],
        p,
    );
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(fs::read(p.join("a.pptx")).unwrap(), left);
}
#[test]
fn image_cli_rejects_unsafe_outputs_unsupported_modes_and_bad_inputs() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    let source = include_bytes!("../../../../demo/yymerge/images/left.png");
    fs::write(p.join("left.png"), source).unwrap();
    fs::write(
        p.join("right.png"),
        include_bytes!("../../../../demo/yymerge/images/right.png"),
    )
    .unwrap();
    fs::write(p.join("keep.html"), "keep").unwrap();
    for extra in [
        vec!["--report", "left.png", "--overwrite"],
        vec!["--diff-image", "left.png", "--overwrite"],
        vec!["--report", "keep.html"],
        vec!["--report", "same.png", "--diff-image", "same.png"],
        vec![
            "--offset-x",
            "40000",
            "--report",
            "keep.html",
            "--overwrite",
        ],
    ] {
        let mut args = vec!["image-diff", "left.png", "right.png"];
        args.extend(extra);
        let out = run(&args, p);
        assert_eq!(
            out.status.code(),
            Some(2),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(out.stdout.is_empty());
        assert_eq!(fs::read(p.join("left.png")).unwrap(), source);
        assert_eq!(fs::read_to_string(p.join("keep.html")).unwrap(), "keep");
    }
    for extra in [
        vec!["--format", "unified"],
        vec!["--ignore-space"],
        vec!["--encoding", "UTF-8"],
    ] {
        let mut args = vec!["diff", "left.png", "right.png"];
        args.extend(extra);
        assert_eq!(run(&args, p).status.code(), Some(2));
    }
    assert_eq!(
        run(
            &["image-diff", "left.png", "right.png", "--threshold", "256"],
            p
        )
        .status
        .code(),
        Some(64)
    );
    fs::write(p.join("bad.png"), b"broken").unwrap();
    assert_eq!(
        run(
            &[
                "image-diff",
                "bad.png",
                "right.png",
                "--report",
                "keep.html",
                "--overwrite"
            ],
            p
        )
        .status
        .code(),
        Some(2)
    );
    assert_eq!(fs::read_to_string(p.join("keep.html")).unwrap(), "keep");
    let out = run(
        &[
            "image-diff",
            "left.png",
            "right.png",
            "--report",
            "keep.html",
            "--diff-image",
            "never.png",
        ],
        p,
    );
    assert_eq!(out.status.code(), Some(2));
    assert!(!p.join("never.png").exists());
    fs::write(p.join("result"), b"keep").unwrap();
    assert_eq!(
        run(
            &[
                "merge",
                "--base",
                "left.png",
                "--left",
                "left.png",
                "--right",
                "right.png",
                "--output",
                "result",
                "--overwrite"
            ],
            p
        )
        .status
        .code(),
        Some(2)
    );
    assert_eq!(fs::read(p.join("result")).unwrap(), b"keep");
    #[cfg(windows)]
    {
        let out = run(
            &[
                "image-diff",
                "left.png",
                "right.png",
                "--report",
                "LEFT.PNG",
                "--diff-image",
                "never.png",
                "--overwrite",
            ],
            p,
        );
        assert_eq!(out.status.code(), Some(2));
        assert!(!p.join("never.png").exists());
        assert_eq!(
            run(
                &[
                    "image-diff",
                    "left.png",
                    "right.png",
                    "--report",
                    "a.png",
                    "--diff-image",
                    "A.png"
                ],
                p
            )
            .status
            .code(),
            Some(2)
        );
    }
}
#[test]
fn office_cli_compares_reports_sessions_and_blocks_source_writes() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    for (ext, a, b, expected) in [
        (
            "xlsx",
            include_bytes!("../../../../demo/yymerge/office/left.xlsx").as_slice(),
            include_bytes!("../../../../demo/yymerge/office/right.xlsx").as_slice(),
            "CELL",
        ),
        (
            "pptx",
            include_bytes!("../../../../demo/yymerge/office/left.pptx").as_slice(),
            include_bytes!("../../../../demo/yymerge/office/right.pptx").as_slice(),
            "SLIDE",
        ),
    ] {
        let left = format!("left.{ext}");
        let right = format!("right.{ext}");
        fs::write(root.join(&left), a).unwrap();
        fs::write(root.join(&right), b).unwrap();
        let out = run(&["diff", &left, &right, "--format", "text"], root);
        assert_eq!(
            out.status.code(),
            Some(1),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(String::from_utf8_lossy(&out.stdout).contains(expected));
        let out = run(&["diff", &left, &left, "--format", "json"], root);
        assert_eq!(out.status.code(), Some(0));
        let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(value["identical"], true);
        assert_eq!(
            value["office"],
            if ext == "xlsx" { "Excel" } else { "PowerPoint" }
        );
        let out = run(
            &[
                "diff",
                &left,
                &right,
                "--format",
                "html",
                "--report",
                "review.html",
                "--overwrite",
            ],
            root,
        );
        assert_eq!(out.status.code(), Some(1));
        let html = fs::read_to_string(root.join("review.html")).unwrap();
        assert!(html.contains("Office抽出データ"));
        assert!(html.contains(expected));
        assert!(!html.contains("<test>"));
        assert_eq!(
            run(
                &[
                    "diff",
                    &left,
                    &right,
                    "--format",
                    "html",
                    "--report",
                    &left,
                    "--overwrite"
                ],
                root
            )
            .status
            .code(),
            Some(2)
        );
        assert_eq!(
            run(
                &[
                    "diff",
                    &left,
                    &right,
                    "--format",
                    "unified",
                    "--report",
                    "review.html",
                    "--overwrite"
                ],
                root
            )
            .status
            .code(),
            Some(2)
        );
        assert_eq!(fs::read_to_string(root.join("review.html")).unwrap(), html);
        assert_eq!(
            run(
                &[
                    "comparison-session",
                    "create",
                    "--left",
                    &left,
                    "--right",
                    &right,
                    "--output",
                    "work.json",
                    "--overwrite"
                ],
                root
            )
            .status
            .code(),
            Some(0)
        );
        assert_eq!(
            run(&["comparison-session", "inspect", "work.json"], root)
                .status
                .code(),
            Some(0)
        );
        fs::write(root.join("result"), "keep").unwrap();
        assert_eq!(
            run(
                &[
                    "merge",
                    "--base",
                    &left,
                    "--left",
                    &left,
                    "--right",
                    &right,
                    "--output",
                    "result",
                    "--overwrite"
                ],
                root
            )
            .status
            .code(),
            Some(2)
        );
        assert_eq!(fs::read_to_string(root.join("result")).unwrap(), "keep");
        assert_eq!(
            run(&["diff", &left, &right, "--encoding", "CP932"], root)
                .status
                .code(),
            Some(2)
        );
        assert_eq!(fs::read(root.join(&left)).unwrap(), a);
        assert_eq!(fs::read(root.join(&right)).unwrap(), b);
        fs::write(root.join(&right), "broken Office package").unwrap();
        let out = run(
            &[
                "diff",
                &left,
                &right,
                "--format",
                "html",
                "--report",
                "review.html",
                "--overwrite",
            ],
            root,
        );
        assert_eq!(out.status.code(), Some(2));
        assert!(out.stdout.is_empty());
        assert_eq!(fs::read_to_string(root.join("review.html")).unwrap(), html);
    }
    assert_eq!(
        run(&["diff", "left.xlsx", "left.pptx"], root).status.code(),
        Some(2)
    );
    fs::write(root.join("old.xls"), b"not text").unwrap();
    assert_eq!(
        run(&["diff", "old.xls", "old.xls"], root).status.code(),
        Some(2)
    );
}
#[test]
fn folder_wildcards_filter_rows_preserve_global_status_and_session_mode() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path();
    for side in ["left", "right", "base"] {
        fs::create_dir_all(path.join(side).join("日本語")).unwrap();
        fs::write(path.join(side).join("日本語/猫.CSV"), "same").unwrap();
    }
    fs::write(path.join("left/change.txt"), "different").unwrap();
    for base in [false, true] {
        let mut args = vec![
            "folders",
            "left",
            "right",
            "--recursive",
            "--path-glob",
            "*日本語/?.csv",
            "--format",
            "json",
        ];
        if base {
            args.extend(["--base", "base"]);
        }
        let out = run(&args, path);
        assert_eq!(
            out.status.code(),
            Some(1),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(value["matched_count"], 1);
        assert_eq!(value["total_differences"], 1);
        assert_eq!(value["filter"]["path_glob"], "*日本語/?.csv");
        assert_eq!(value["filter"]["path_contains"], "");
    }
    let out = run(
        &[
            "folders",
            "left",
            "right",
            "--recursive",
            "--path-glob",
            "*.absent",
            "--format",
            "json",
        ],
        path,
    );
    assert_eq!(out.status.code(), Some(1));
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["matched_count"], 0);
    assert_eq!(
        run(
            &[
                "folders",
                "left",
                "right",
                "--path-glob",
                "*.csv",
                "--path-contains",
                "csv"
            ],
            path
        )
        .status
        .code(),
        Some(64)
    );
    fs::write(path.join("existing.json"), "keep").unwrap();
    for pattern in ["".into(), "?".repeat(257)] {
        let out = run(
            &[
                "folders",
                "left",
                "right",
                "--path-glob",
                &pattern,
                "--report",
                "existing.json",
                "--overwrite",
            ],
            path,
        );
        assert_eq!(out.status.code(), Some(2));
        assert!(out.stdout.is_empty());
        assert_eq!(
            fs::read_to_string(path.join("existing.json")).unwrap(),
            "keep"
        );
    }
    assert_eq!(
        run(
            &[
                "folder-session",
                "create",
                "--left",
                "left",
                "--right",
                "right",
                "--path-glob",
                "*.csv",
                "--output",
                "work.json"
            ],
            path
        )
        .status
        .code(),
        Some(0)
    );
    let out = run(&["folder-session", "inspect", "work.json"], path);
    assert_eq!(out.status.code(), Some(0));
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["path_glob"], "*.csv");
    assert_eq!(value["path_contains"], "");
}
#[test]
fn folder_sessions_capture_setup_protect_roots_and_do_not_store_results() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path();
    for side in ["left", "right", "base"] {
        fs::create_dir_all(path.join(side).join("nested")).unwrap();
    }
    fs::write(path.join("left/new.csv"), "changed").unwrap();
    let args = [
        "folder-session",
        "create",
        "--left",
        "left",
        "--right",
        "right",
        "--base",
        "base",
        "--recursive",
        "--path-contains",
        ".csv",
        "--status",
        "different",
        "--output",
        "folders.json",
    ];
    let out = run(&args, path);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let before = fs::read(path.join("folders.json")).unwrap();
    let out = run(&["folder-session", "inspect", "folders.json"], path);
    assert_eq!(out.status.code(), Some(0));
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["kind"], "yymerge-folder-session");
    assert_eq!(value["recursive"], true);
    assert_eq!(value["status"], "different");
    assert_eq!(value["path_contains"], ".csv");
    assert_eq!(value["results_restored"], false);
    assert_eq!(value["sync_plan_restored"], false);
    assert_eq!(run(&args, path).status.code(), Some(2));
    assert_eq!(fs::read(path.join("folders.json")).unwrap(), before);
    for target in ["left/work.json", "right/nested/work.json", "base/work.json"] {
        assert_eq!(
            run(
                &[
                    "folder-session",
                    "create",
                    "--left",
                    "left",
                    "--right",
                    "right",
                    "--base",
                    "base",
                    "--output",
                    target,
                    "--overwrite"
                ],
                path
            )
            .status
            .code(),
            Some(2)
        );
        assert!(!path.join(target).exists());
    }
    fs::write(path.join("right/new.csv"), "changed").unwrap();
    assert_eq!(
        run(&["folder-session", "inspect", "folders.json"], path)
            .status
            .code(),
        Some(0)
    );
    fs::rename(path.join("base"), path.join("moved-base")).unwrap();
    let out = run(&["folder-session", "inspect", "folders.json"], path);
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
    assert_eq!(fs::read(path.join("folders.json")).unwrap(), before);
}
#[test]
fn folder_sessions_invalid_json_and_gui_conflicting_modes_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path();
    fs::write(path.join("bad.json"), "{}").unwrap();
    let out = run(&["folder-session", "inspect", "bad.json"], path);
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
    for flags in [
        &["--session", "merge.json"][..],
        &["--comparison-session", "compare.json"][..],
        &["--left", "left", "--right", "right"][..],
        &["--ignore-case"][..],
        &["--encoding", "UTF-8"][..],
    ] {
        let mut args = vec!["gui", "--folder-session", "bad.json"];
        args.extend(flags);
        assert_eq!(run(&args, path).status.code(), Some(64));
    }
}
#[test]
fn comparison_sessions_preserve_rules_protect_inputs_and_validate_before_inspection() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path();
    fs::write(path.join("left"), "猫 Alpha\r\n").unwrap();
    fs::write(path.join("right"), "猫 alpha\n").unwrap();
    let args = [
        "comparison-session",
        "create",
        "--left",
        "left",
        "--right",
        "right",
        "--output",
        "work.json",
        "--ignore-case",
        "--ignore-eol",
    ];
    let out = run(&args, path);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let before = fs::read(path.join("work.json")).unwrap();
    let out = run(&["comparison-session", "inspect", "work.json"], path);
    assert_eq!(out.status.code(), Some(0));
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["kind"], "yymerge-comparison-session");
    assert_eq!(value["inputs_verified"], true);
    assert_eq!(value["rules"]["ignore_case"], true);
    assert_eq!(value["differences"], 0);
    assert_eq!(value["ignored"], 1);
    assert!(!value["left"].as_str().unwrap().is_empty());
    assert_eq!(run(&args, path).status.code(), Some(2));
    assert_eq!(fs::read(path.join("work.json")).unwrap(), before);
    for target in ["left", "right"] {
        let out = run(
            &[
                "comparison-session",
                "create",
                "--left",
                "left",
                "--right",
                "right",
                "--output",
                target,
                "--overwrite",
            ],
            path,
        );
        assert_eq!(out.status.code(), Some(2));
    }
    assert_eq!(
        fs::read_to_string(path.join("left")).unwrap(),
        "猫 Alpha\r\n"
    );
    fs::write(path.join("left"), "changed").unwrap();
    let out = run(&["comparison-session", "inspect", "work.json"], path);
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
    assert_eq!(fs::read(path.join("work.json")).unwrap(), before);
}
#[test]
fn comparison_sessions_profiles_legacy_encodings_and_gui_exclusions() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path();
    fs::write(path.join("left"), [0x94, 0x4c, b'A']).unwrap();
    fs::write(path.join("right"), [0x94, 0x4c, b'a']).unwrap();
    let out = run(
        &["rules-profile", "save", "rules.json", "--ignore-case"],
        path,
    );
    assert_eq!(out.status.code(), Some(0));
    let profile = fs::read(path.join("rules.json")).unwrap();
    let create = |target| {
        run(
            &[
                "comparison-session",
                "create",
                "--left",
                "left",
                "--right",
                "right",
                "--encoding",
                "CP932",
                "--rules-profile",
                "rules.json",
                "--output",
                target,
                "--overwrite",
            ],
            path,
        )
    };
    assert_eq!(create("rules.json").status.code(), Some(2));
    assert_eq!(fs::read(path.join("rules.json")).unwrap(), profile);
    assert_eq!(create("work.json").status.code(), Some(0));
    let out = run(&["comparison-session", "inspect", "work.json"], path);
    assert_eq!(out.status.code(), Some(0));
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["differences"], 0);
    fs::write(path.join("bad.json"), "{}").unwrap();
    let out = run(&["comparison-session", "inspect", "bad.json"], path);
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
    for options in [
        &["--session", "merge.json"][..],
        &["--left", "left", "--right", "right"][..],
        &["--encoding", "CP932"][..],
        &["--ignore-case"][..],
        &["--rules-profile", "rules.json"][..],
    ] {
        let mut args = vec!["gui", "--comparison-session", "work.json"];
        args.extend(options);
        assert_eq!(run(&args, path).status.code(), Some(64));
    }
}
#[test]
fn folder_filters_report_visible_rows_and_keep_the_global_exit_status() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path();
    for root in ["left", "right"] {
        fs::create_dir_all(path.join(root).join("日本語")).unwrap();
        fs::write(path.join(root).join("same.txt"), "same").unwrap();
        fs::write(path.join(root).join("日本語/Data.CSV"), root).unwrap();
    }
    fs::write(path.join("left/only-left.txt"), "left").unwrap();
    let out = run(
        &[
            "folders",
            "left",
            "right",
            "--recursive",
            "--path-contains",
            "日本語\\DATA.csv",
            "--status",
            "different",
            "--format",
            "json",
        ],
        path,
    );
    assert_eq!(out.status.code(), Some(1));
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["version"], 1);
    assert_eq!(value["matched_count"], 1);
    assert_eq!(value["total_count"], 4);
    assert_eq!(value["total_differences"], 2);
    assert_eq!(value["entries"][0]["status"], "Different");
    assert_eq!(value["filter"]["status"], "different");
    let out = run(
        &[
            "folders",
            "left",
            "right",
            "--recursive",
            "--status",
            "equal",
            "--path-contains",
            "same.txt",
            "--format",
            "json",
        ],
        path,
    );
    assert_eq!(out.status.code(), Some(1));
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["entries"].as_array().unwrap().len(), 1);
    let out = run(
        &[
            "folders",
            "left",
            "right",
            "--recursive",
            "--path-contains",
            "no-match",
            "--format",
            "json",
        ],
        path,
    );
    assert_eq!(out.status.code(), Some(1));
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["matched_count"], 0);
    assert_eq!(value["total_differences"], 2);
    let out = run(
        &[
            "folders",
            "left",
            "right",
            "--recursive",
            "--format",
            "json",
        ],
        path,
    );
    assert!(
        serde_json::from_slice::<serde_json::Value>(&out.stdout)
            .unwrap()
            .is_array()
    );
    let out = run(&["folders", "left", "right", "--status", "left-only"], path);
    assert!(
        String::from_utf8(out.stdout)
            .unwrap()
            .contains("LeftOnly\tonly-left.txt")
    );
    assert_eq!(
        run(
            &["folders", "left", "right", "--status", "not-a-status"],
            path
        )
        .status
        .code(),
        Some(64)
    );
}
#[test]
fn three_way_folder_review_and_presence_filters_do_not_restrict_sync_planning() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path();
    for root in ["base", "left", "right"] {
        fs::create_dir(path.join(root)).unwrap();
    }
    for (root, value) in [("base", "base"), ("left", "left"), ("right", "right")] {
        fs::write(path.join(root).join("both.txt"), value).unwrap();
        fs::write(path.join(root).join("same.txt"), "same").unwrap();
    }
    fs::write(path.join("left/added.txt"), "new").unwrap();
    let out = run(
        &[
            "folders", "left", "right", "--base", "base", "--status", "review", "--format", "json",
        ],
        path,
    );
    assert_eq!(out.status.code(), Some(1));
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["matched_count"], 1);
    assert_eq!(value["total_differences"], 2);
    assert_eq!(value["entries"][0]["status"], "BothChanged");
    let out = run(
        &[
            "folders",
            "left",
            "right",
            "--base",
            "base",
            "--status",
            "left-only",
            "--format",
            "json",
        ],
        path,
    );
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["entries"][0]["status"], "LeftAdded");
    let out = run(
        &[
            "folders",
            "left",
            "right",
            "--base",
            "base",
            "--path-contains",
            "missing",
            "--format",
            "json",
        ],
        path,
    );
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&out.stdout).unwrap()["matched_count"],
        0
    );
    let out = run(
        &[
            "sync", "plan", "--left", "left", "--right", "right", "--base", "base", "--mode",
            "merge",
        ],
        path,
    );
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        value["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["path"] == "added.txt")
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
                "--path-contains",
                "missing"
            ],
            path
        )
        .status
        .code(),
        Some(64)
    );
}
#[test]
fn filtered_incomplete_rows_never_become_empty_success_or_hide_global_errors() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path();
    for root in ["base", "left", "right"] {
        fs::create_dir(path.join(root)).unwrap();
        fs::write(path.join(root).join("same.txt"), "same").unwrap();
        fs::write(path.join(root).join("bad.txt"), "bad").unwrap();
    }
    #[cfg(windows)]
    let _held = {
        use std::os::windows::fs::OpenOptionsExt;
        fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(path.join("left/bad.txt"))
            .unwrap()
    };
    #[cfg(unix)]
    {
        fs::remove_file(path.join("left/bad.txt")).unwrap();
        std::os::unix::fs::symlink("missing", path.join("left/bad.txt")).unwrap();
    }
    for base in [false, true] {
        let mut args = vec![
            "folders",
            "left",
            "right",
            "--path-contains",
            "no-match",
            "--format",
            "json",
        ];
        if base {
            args.extend(["--base", "base"]);
        }
        let out = run(&args, path);
        assert_eq!(out.status.code(), Some(2));
        let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(value["matched_count"], 0);
        assert_eq!(value["total_incomplete"], 1);
    }
    let out = run(
        &[
            "folders",
            "left",
            "right",
            "--status",
            "uncompared",
            "--format",
            "json",
        ],
        path,
    );
    assert_eq!(out.status.code(), Some(2));
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["matched_count"], 1);
    assert!(
        value["entries"][0]["status"] == "Error" || value["entries"][0]["status"] == "SkippedLink"
    );
}
#[test]
fn merge_work_sessions_round_trip_and_never_write_a_result_or_input() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path();
    for (name, value) in [
        ("base", "a\n"),
        ("left", "LEFT\n"),
        ("right", "RIGHT\n"),
        ("result", "keep"),
    ] {
        fs::write(path.join(name), value).unwrap();
    }
    let create = [
        "session",
        "create",
        "--base",
        "base",
        "--left",
        "left",
        "--right",
        "right",
        "--output",
        "作業 session.json",
    ];
    assert_eq!(run(&create, path).status.code(), Some(0));
    let out = run(&["session", "inspect", "作業 session.json"], path);
    assert_eq!(out.status.code(), Some(0));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["unresolved"], 1);
    assert_eq!(json["inputs_verified"], true);
    assert_eq!(json["merge_result_saved"], false);
    assert_eq!(json["undo_history_restored"], false);
    assert_eq!(fs::read_to_string(path.join("result")).unwrap(), "keep");
    let before = fs::read(path.join("作業 session.json")).unwrap();
    assert_eq!(run(&create, path).status.code(), Some(2));
    assert_eq!(fs::read(path.join("作業 session.json")).unwrap(), before);
    let overwrite = [
        "session",
        "create",
        "--base",
        "base",
        "--left",
        "left",
        "--right",
        "right",
        "--output",
        "left",
        "--overwrite",
    ];
    assert_eq!(run(&overwrite, path).status.code(), Some(2));
    assert_eq!(fs::read_to_string(path.join("left")).unwrap(), "LEFT\n");
    fs::write(path.join("left"), "changed externally\n").unwrap();
    let out = run(&["session", "inspect", "作業 session.json"], path);
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
    assert!(String::from_utf8(out.stderr).unwrap().contains("changed"));
    assert_eq!(fs::read(path.join("作業 session.json")).unwrap(), before);
}
#[test]
fn invalid_and_missing_session_inputs_are_incomplete_and_mixed_bom_cannot_be_saved() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path();
    fs::write(
        path.join("session.json"),
        "{\"kind\":\"other\",\"version\":99}",
    )
    .unwrap();
    assert_eq!(
        run(&["session", "inspect", "session.json"], path)
            .status
            .code(),
        Some(2)
    );
    for name in ["base", "left", "right"] {
        fs::write(path.join(name), "same\r\n").unwrap();
    }
    let args = [
        "session",
        "create",
        "--base",
        "base",
        "--left",
        "left",
        "--right",
        "right",
        "--output",
        "valid.json",
    ];
    assert_eq!(run(&args, path).status.code(), Some(0));
    fs::remove_file(path.join("right")).unwrap();
    assert_eq!(
        run(&["session", "inspect", "valid.json"], path)
            .status
            .code(),
        Some(2)
    );
    fs::write(path.join("right"), "\u{feff}same\r\n").unwrap();
    assert_eq!(run(&args, path).status.code(), Some(2));
    assert!(!path.join("result").exists());
    assert_eq!(
        run(&["gui", "--session", "valid.json", "--ignore-case"], path)
            .status
            .code(),
        Some(64)
    );
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
