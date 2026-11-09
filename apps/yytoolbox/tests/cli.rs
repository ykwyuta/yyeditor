use std::ffi::OsString;
use yytoolbox::cli::run;

fn cli(args: &[&str], stdin: &[u8]) -> (i32, String, String) {
    let mut out = Vec::new();
    let mut err = Vec::new();
    let args: Vec<OsString> = args.iter().map(OsString::from).collect();
    let code = run(args, &mut &stdin[..], &mut out, &mut err);
    (
        code,
        String::from_utf8(out).unwrap(),
        String::from_utf8(err).unwrap(),
    )
}

#[test]
fn help_list_and_every_tool_id_round_trips() {
    let (code, out, _) = cli(&[], b"");
    assert_eq!(code, 0);
    assert!(out.contains("steps"));
    let (code, out, _) = cli(&["list"], b"");
    assert_eq!(code, 0);
    for tool in yytoolbox::TOOLS {
        assert_eq!(yytoolbox::Tool::from_id(tool.id()), Some(tool));
        assert!(out.contains(tool.id()), "{}", tool.id());
        let (code, help, _) = cli(&[tool.id(), "--help"], b"");
        assert_eq!(code, 0, "{}", tool.id());
        assert!(help.contains(tool.label()));
    }
    let (code, _, err) = cli(&["no-such-tool"], b"");
    assert_eq!(code, 2);
    assert!(err.contains("no-such-tool"));
}

#[test]
fn tools_read_stdin_text_and_files_and_validate_options() {
    let (code, out, _) = cli(&["base64-encode"], "こんにちは".as_bytes());
    assert_eq!((code, out.as_str()), (0, "44GT44KT44Gr44Gh44Gv\n"));
    let (code, out, _) = cli(&["base64-decode", "--text", "44GT44KT44Gr44Gh44Gv"], b"");
    assert_eq!((code, out.as_str()), (0, "こんにちは\n"));
    let (code, out, _) = cli(&["hash", "--algorithm=SHA-256", "--text", "abc"], b"");
    assert_eq!(code, 0);
    assert!(out.contains("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"));
    let (code, out, _) = cli(&["json", "--layout", "圧縮"], br#"{ "a" : [1, 2] }"#);
    assert_eq!((code, out.trim()), (0, r#"{"a":[1,2]}"#));
    let (code, out, _) = cli(&["password", "--length", "12", "--count", "3"], b"");
    assert_eq!(code, 0);
    assert_eq!(out.lines().filter(|l| l.chars().count() == 12).count(), 3);

    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("in.json");
    // Shift_JIS input is detected like the GUI's file opening.
    std::fs::write(&input, b"{\"k\":\"\x83e\x83X\x83g\"}").unwrap();
    let saved = dir.path().join("out.json");
    let (code, out, _) = cli(
        &[
            "json",
            "--input",
            input.to_str().unwrap(),
            "--output",
            saved.to_str().unwrap(),
        ],
        b"",
    );
    assert_eq!((code, out.as_str()), (0, ""));
    assert!(std::fs::read_to_string(&saved).unwrap().contains("テスト"));

    for bad in [
        &["hash", "--algorithm", "SHA-999", "--text", "x"][..],
        &["hash", "--nope", "x", "--text", "x"],
        &["hash", "--text"],
        &["hash", "--text", "a", "--example"],
        &["password", "--text", "a"],
        &["hash", "extra"],
    ] {
        assert_eq!(cli(bad, b"").0, 2, "{bad:?}");
    }
    let (code, _, err) = cli(&["recording"], b"");
    assert_eq!(code, 1);
    assert!(err.contains("画面"));
    let (code, _, err) = cli(&["json", "--text", "{"], b"");
    assert_eq!(code, 1);
    assert!(!err.is_empty());
    let (code, out, _) = cli(&["calendar", "--example"], b"");
    assert_eq!(code, 0);
    assert!(out.contains("2019"));
}

#[test]
fn steps_counts_directories_with_languages_encodings_and_excludes() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("src/sub")).unwrap();
    std::fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::write(
        root.join("src/Main.java"),
        "/**\n * doc\n */\npublic class Main {\n\n    // c\n    void f() {}\n}\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/sub/app.py"),
        "# c\nimport os\n\n\"\"\"doc\"\"\"\nprint('#')\n",
    )
    .unwrap();
    // Shift_JIS: "表" ends with 0x5C, which must not escape the closing quote.
    let mut sjis = b"char *s = \"\x95\x5C\"; /* \x83R\x83\x81\x83\x93\x83g */\n".to_vec();
    sjis.extend(b"/* c\n */\nint x;\r\n");
    std::fs::write(root.join("src/sub/legacy.c"), sjis).unwrap();
    std::fs::write(root.join("src/notes.txt"), "not code\n").unwrap();
    std::fs::write(root.join("node_modules/pkg/index.js"), "x();\n").unwrap();
    std::fs::write(root.join(".git/hook.sh"), "x\n").unwrap();
    let path = root.to_str().unwrap();

    let (code, out, err) = cli(&["steps", "--format", "json", path], b"");
    assert_eq!(code, 0, "{err}");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let files = v["files"].as_array().unwrap();
    assert_eq!(files.len(), 3, "{out}");
    let get = |suffix: &str| {
        files
            .iter()
            .find(|f| f["path"].as_str().unwrap().ends_with(suffix))
            .unwrap()
            .clone()
    };
    let java = get("Main.java");
    assert_eq!(
        (
            &java["code"],
            &java["comment"],
            &java["blank"],
            &java["total"]
        ),
        (&3.into(), &4.into(), &1.into(), &8.into())
    );
    let py = get("app.py");
    assert_eq!(
        (&py["code"], &py["comment"], &py["blank"]),
        (&2.into(), &2.into(), &1.into())
    );
    let c = get("legacy.c");
    assert_eq!((&c["code"], &c["comment"]), (&2.into(), &2.into()));
    assert_ne!(c["encoding"], "UTF-8");
    assert_eq!(v["total"]["code"], 7);
    assert_eq!(v["total"]["files"], 3);

    // Text output has a language table and optional per-file details.
    let (code, out, _) = cli(&["steps", "--files", path], b"");
    assert_eq!(code, 0);
    assert!(out.contains("Java") && out.contains("合計") && out.contains("Main.java"));

    // Excludes, default excludes and hidden directories.
    let (_, out, _) = cli(&["steps", "--format", "csv", "--exclude", "sub", path], b"");
    assert_eq!(out.lines().count(), 3, "{out}");
    let (_, out, _) = cli(
        &[
            "steps",
            "--format",
            "csv",
            "--exclude",
            "*.py",
            "--no-default-excludes",
            path,
        ],
        b"",
    );
    assert!(out.contains("index.js") && !out.contains("app.py") && !out.contains("hook.sh"));
    assert!(out.starts_with("path,language,encoding,total,code,comment,blank\r\n"));

    // An explicitly named file with an unknown extension is reported, a forced language counts it.
    let notes = root.join("src/notes.txt");
    let (code, _, err) = cli(&["steps", notes.to_str().unwrap()], b"");
    assert_eq!(code, 0);
    assert!(err.contains("notes.txt"));
    let (_, out, _) = cli(
        &[
            "steps",
            "--lang",
            "python",
            "--format",
            "csv",
            notes.to_str().unwrap(),
        ],
        b"",
    );
    assert!(out.contains(",Python,"));

    let saved = root.join("steps.json");
    let (code, out, _) = cli(
        &[
            "steps",
            "--format",
            "json",
            "--output",
            saved.to_str().unwrap(),
            path,
        ],
        b"",
    );
    assert_eq!((code, out.as_str()), (0, ""));
    assert!(
        std::fs::read_to_string(saved)
            .unwrap()
            .contains("\"total\"")
    );
}

#[test]
fn steps_from_stdin_and_errors() {
    let (code, out, _) = cli(
        &["steps", "--lang", "cobol", "--format", "csv"],
        b"000100 IDENTIFICATION DIVISION.\n000200* COMMENT\n",
    );
    assert_eq!(code, 0);
    assert!(out.contains("-,COBOL,UTF-8,2,1,1,0"), "{out}");
    assert_eq!(cli(&["steps"], b"x").0, 2);
    assert_eq!(cli(&["steps", "--lang", "klingon", "-"], b"x").0, 2);
    assert_eq!(cli(&["steps", "--format", "xml", "--lang", "c"], b"x").0, 2);
    assert_eq!(cli(&["steps", "/definitely/missing/path"], b"").0, 1);
    let (code, out, _) = cli(&["steps", "--languages"], b"");
    assert_eq!(code, 0);
    for name in ["Java", "COBOL", "Python", "C#", "Visual Basic", ".rs"] {
        assert!(out.contains(name), "{name}");
    }
    // The GUI tool counts the input text with the selected language.
    let (code, out, _) = cli(&["steps", "--help"], b"");
    assert_eq!(code, 0);
    assert!(out.contains("--lang"));
}
