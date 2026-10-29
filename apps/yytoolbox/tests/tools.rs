use yytoolbox::{Params, Tool, execute};
fn run(tool: Tool, input: &str) -> yytoolbox::Output {
    execute(tool, input, &Params::new(), &|| false).unwrap()
}

#[test]
fn base64_unicode_binary_and_url_safe() {
    let encoded = run(Tool::Base64Encode, "こんにちは😀");
    let decoded = run(Tool::Base64Decode, &encoded.text);
    assert_eq!(decoded.text, "こんにちは😀");
    let binary = run(Tool::Base64Decode, " /w A=\n");
    assert_eq!(binary.bytes, vec![255, 0]);
    assert_eq!(binary.filename, "decoded.bin");
    let mut p = Params::new();
    p.insert("alphabet".into(), "URL-safe".into());
    let encoded = execute(Tool::Base64Encode, "?? > 日本語", &p, &|| false).unwrap();
    assert!(!encoded.text.contains(['+', '/', '=']));
    assert_eq!(
        execute(Tool::Base64Decode, &encoded.text, &p, &|| false)
            .unwrap()
            .text,
        "?? > 日本語"
    );
    assert!(execute(Tool::Base64Decode, "%%%", &p, &|| false).is_err());
}

#[test]
fn known_hash_vectors() {
    let out = run(Tool::Hash, "abc");
    for expected in [
        "MD5: 900150983cd24fb0d6963f7d28e17f72",
        "SHA-1: a9993e364706816aba3e25717850c26c9cd0d89d",
        "SHA-256: ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        "CRC32: 352441c2",
    ] {
        assert!(out.text.contains(expected), "{expected}");
    }
}

#[test]
fn json_preserves_big_numbers_escapes_order_and_rejects_invalid_input() {
    let source = r#"{ "z":123456789012345678901234567890, "a":1e400, "s":"a\\\"b", "empty":[],"object":{},"n":-0.00001 }"#;
    let out = run(Tool::Json, source);
    assert!(out.text.contains("123456789012345678901234567890"));
    assert!(out.text.contains("1e400"));
    assert!(out.text.find("\"z\"").unwrap() < out.text.find("\"a\"").unwrap());
    let mut p = Params::new();
    p.insert("layout".into(), "圧縮".into());
    let compact = execute(Tool::Json, &out.text, &p, &|| false).unwrap();
    assert_eq!(
        execute(Tool::Json, &compact.text, &Params::new(), &|| false)
            .unwrap()
            .text,
        out.text
    );
    assert!(execute(Tool::Json, "{\"a\":1,}", &p, &|| false).is_err());
    assert!(execute(Tool::Json, "{} {}", &p, &|| false).is_err());
}

#[test]
fn regex_named_unicode_zero_length_and_invalid_patterns() {
    let p = Tool::Regex.defaults();
    let out = execute(Tool::Regex, "日本語 ABC", &p, &|| false).unwrap();
    let matches: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    assert_eq!(matches[0]["groups"][0]["end_byte"], 9);
    assert_eq!(matches[0]["groups"][1]["name"], "word");
    let mut p = Params::new();
    p.insert("pattern".into(), "".into());
    assert!(execute(Tool::Regex, "日本語", &p, &|| false).is_ok());
    p.insert("pattern".into(), "(".into());
    assert!(execute(Tool::Regex, "text", &p, &|| false).is_err());
}

#[test]
fn jwt_is_decode_only_and_sql_preserves_literals() {
    let tool = Tool::Jwt;
    let out = run(tool, &tool.example(&tool.defaults()));
    let value: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    assert_eq!(value["signature_verified"], false);
    assert_eq!(value["payload"]["sub"], "test-user");
    assert!(execute(Tool::Jwt, "a.b.c.d.e", &Params::new(), &|| false).is_err());
    let out = run(
        Tool::Sql,
        "select 'MiXeD  text', id from t where x = '日本語';",
    );
    assert!(out.text.contains("'MiXeD  text'"));
    assert!(out.text.contains("'日本語'"));
}

#[test]
fn every_tool_has_an_executable_default_example_and_limits_are_enforced() {
    for tool in yytoolbox::TOOLS {
        let p = tool.defaults();
        if matches!(tool, Tool::Recording | Tool::ResizeImages | Tool::VideoEdit) {
            assert!(
                execute(tool, "", &p, &|| false)
                    .unwrap_err()
                    .contains("Windows画面")
            );
            continue;
        }
        execute(tool, &tool.example(&p), &p, &|| false).unwrap_or_else(|e| panic!("{tool:?}: {e}"));
    }
    assert!(
        execute(
            Tool::Hash,
            &"x".repeat(yytoolbox::MAX_INPUT + 1),
            &Params::new(),
            &|| false
        )
        .is_err()
    );
    assert!(execute(Tool::Hash, "text", &Params::new(), &|| true).is_err());
}

#[test]
fn dummy_yaml_preserves_string_values_and_reports_definition_errors() {
    let source = r#"# 列定義。生成件数は画面の「行数」で指定する
columns:
  - name: ID
    type: sequence
    prefix: TEST-
    width: 3
  - name: 郵便番号
    type: literal
    value: '001'
  - name: フラグ文字列
    type: choice
    values: ['true', 'false']
  - name: メモ
    type: literal
    value: |-
      一行目,カンマ
      "二行目"
"#;
    let mut params = Tool::DummyCsv.defaults();
    params.insert("rows".into(), "3".into());
    let out = execute(Tool::DummyCsv, source, &params, &|| false).unwrap();
    let rows: Vec<_> = csv::Reader::from_reader(out.text.as_bytes())
        .records()
        .map(Result::unwrap)
        .collect();
    assert_eq!(rows.len(), 3);
    assert_eq!(&rows[0][0], "TEST-001");
    for row in &rows {
        assert_eq!(&row[1], "001");
        assert!(matches!(&row[2], "true" | "false"));
        assert_eq!(&row[3], "一行目,カンマ\n\"二行目\"");
    }
    for bad in [
        "columns: [",
        "columns:\n\t- name: tab\n",
        "columns: []\ncolumns: []\n",
        "columns: []\n---\ncolumns: []\n",
        "columns:\n- name: X\n  type: literal\n  value: x\n  unknown_field: 1\n",
        "columns:\n- name: X\n  name: Y\n  type: literal\n  value: x\n",
    ] {
        let error = execute(Tool::DummyCsv, bad, &params, &|| false).unwrap_err();
        assert!(error.starts_with("列定義YAML:"), "{error}");
    }
}
