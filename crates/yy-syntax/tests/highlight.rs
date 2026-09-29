//! シンタックスハイライト（10 章 10 のテスト）。

use std::path::Path;
use std::sync::Arc;

use proptest::prelude::*;
use yy_buffer::Snapshot;
use yy_syntax::{LineState, Registry, Syntax, SyntaxIndex};

/// 行ごとの「トークン名:テキスト」の列（トークンのない部分は出さない）。
fn tokens(s: &Syntax, text: &str) -> Vec<Vec<String>> {
    let mut st = LineState::default();
    let mut out = Vec::new();
    for line in text.lines() {
        let mut spans = Vec::new();
        st = s.highlight_line(&st, line.as_bytes(), &mut spans);
        out.push(
            spans
                .iter()
                .map(|t| {
                    format!(
                        "{}:{}",
                        s.token_name(t.token),
                        &line[t.range.start as usize..t.range.end as usize]
                    )
                })
                .collect(),
        );
    }
    out
}

fn get(id: &str) -> Arc<Syntax> {
    static REG: std::sync::OnceLock<Registry> = std::sync::OnceLock::new();
    REG.get_or_init(Registry::builtin).get(id).unwrap()
}

#[test]
fn every_builtin_definition_compiles_and_runs() {
    let reg = Registry::builtin();
    let sample = "int main() { /* comment */ return \"str\" + 'c' + 42; } // end\n\
                  # heading <tag attr=\"v\"> SELECT * FROM t; \t  IDENTIFICATION DIVISION.\n\
                  ${VAR} $x @at 0x1F 3.14e-2 `code` [link](url) -- sql comment\n";
    let list = reg.list();
    assert!(list.len() >= 37, "{}", list.len());
    for (id, name) in list {
        let s = reg.get(&id).unwrap_or_else(|e| panic!("{id}: {e}"));
        assert_eq!(s.name, name);
        let t = tokens(&s, sample);
        assert_eq!(t.len(), 3, "{id}");
    }
}

#[test]
fn c_tokens() {
    let c = get("c");
    let t = tokens(
        &c,
        "if (x) { printf(\"a\\\"b\", 'c'); } // done\n/* multi\nline */ int n = 0x1F;\n#include <stdio.h>",
    );
    assert_eq!(
        t[0],
        [
            "keyword:if",
            "function:printf",
            "string:\"a\\\"b\"",
            "string:'c'",
            "comment:// done"
        ]
    );
    assert_eq!(t[1], ["comment:/* multi"]);
    assert_eq!(t[2], ["comment:line */", "type:int", "number:0x1F"]);
    assert_eq!(t[3], ["preprocessor:#include"]);
}

#[test]
fn python_triple_quoted_string_spans_lines() {
    let py = get("python");
    let t = tokens(
        &py,
        "def f(x):\n    \"\"\"doc\n    more\"\"\"\n    return None # c",
    );
    assert_eq!(t[0], ["keyword:def", "function:f"]);
    assert_eq!(t[1], ["string:\"\"\"doc"]);
    assert_eq!(t[2], ["string:    more\"\"\""]);
    assert_eq!(t[3], ["keyword:return", "constant:None", "comment:# c"]);
}

#[test]
fn html_tags_and_attributes() {
    let h = get("html");
    let t = tokens(&h, "<a href=\"x\">text</a><!-- c -->");
    assert_eq!(
        t[0],
        [
            "punctuation:<",
            "tag:a",
            "punctuation: ",
            "attribute:href",
            "punctuation:=",
            "string:\"x\"",
            "punctuation:>",
            "punctuation:</",
            "tag:a",
            "punctuation:>",
            "comment:<!-- c -->"
        ]
    );
}

#[test]
fn cobol_fixed_columns() {
    let cob = get("cobol");
    let src = "000100 IDENTIFICATION DIVISION.                                         PROG0001\n\
               000200*THIS IS A COMMENT LINE\n\
               000300     MOVE 'ABC' TO WS-NAME.\n\
               000400 01  WS-NAME PIC X(10).";
    let t = tokens(&cob, src);
    assert_eq!(
        t[0],
        [
            "line-number:000100",
            "keyword:IDENTIFICATION",
            "keyword:DIVISION",
            "comment:PROG0001"
        ]
    );
    assert_eq!(t[1], ["comment:000200*THIS IS A COMMENT LINE"]);
    assert_eq!(
        t[2],
        [
            "line-number:000300",
            "keyword:MOVE",
            "string:'ABC'",
            "keyword:TO"
        ]
    );
    assert_eq!(
        t[3],
        [
            "line-number:000400",
            "number:01",
            "keyword:PIC",
            "type:X(10)."
        ]
    );
}

#[test]
fn jcl_statements_and_data() {
    let jcl = get("jcl");
    let t = tokens(
        &jcl,
        "//MYJOB    JOB (ACCT),'NAME',CLASS=A\n//* COMMENT\n//STEP1    EXEC PGM=IEFBR14\nDATA LINE",
    );
    assert_eq!(
        t[0],
        [
            "punctuation://",
            "label:MYJOB",
            "keyword:JOB",
            "string:'NAME'",
            "property:CLASS"
        ]
    );
    assert_eq!(t[1], ["comment://* COMMENT"]);
    assert_eq!(t[3], ["data:DATA LINE"]);
}

#[test]
fn diff_lines() {
    let d = get("diff");
    let t = tokens(&d, "--- a/x\n+++ b/x\n@@ -1 +1 @@\n-old\n+new\n context");
    assert_eq!(t[0], ["meta:--- a/x"]);
    assert_eq!(t[2], ["section:@@ -1 +1 @@"]);
    assert_eq!(t[3], ["deleted:-old"]);
    assert_eq!(t[4], ["inserted:+new"]);
    assert!(t[5].is_empty());
}

#[test]
fn single_line_definitions_need_no_index() {
    // 範囲がすべて行末で終わる定義は行をまたぐ状態を持たない
    for id in ["cobol", "log", "ini", "diff", "batch"] {
        assert!(!get(id).is_multiline(), "{id}");
    }
    for id in ["c", "python", "html", "markdown", "sql"] {
        assert!(get(id).is_multiline(), "{id}");
    }
}

#[test]
fn detects_file_types() {
    let reg = Registry::builtin();
    let d = |p: &str, head: &str| reg.detect(Some(Path::new(p)), head.as_bytes());
    assert_eq!(d("main.rs", "").as_deref(), Some("rust"));
    assert_eq!(d("Makefile", "").as_deref(), Some("makefile"));
    assert_eq!(d("types.d.ts", "").as_deref(), Some("typescript"));
    assert_eq!(d("A.CBL", "").as_deref(), Some("cobol"));
    assert_eq!(
        d("script", "#!/usr/bin/env python3\n").as_deref(),
        Some("python")
    );
    assert_eq!(d("x.txt", "# vim: ft=ruby\n").as_deref(), Some("ruby"));
    assert_eq!(d("page", "<!DOCTYPE html>\n").as_deref(), Some("html"));
    assert_eq!(d("notes.txt", "hello"), None);
    // 別名・表示名でも引ける
    assert_eq!(reg.get("c++").unwrap().id, "cpp");
}

#[test]
fn user_definitions_override_builtin() {
    let dir = std::env::temp_dir().join(format!("yy-syntax-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("mylang.toml"),
        "name = \"MyLang\"\nextensions = [\"my\"]\n[keywords]\nkeyword = [\"foo\"]\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("broken.toml"),
        "name = \"X\"\n[[context.main]]\nmatch = '('\n",
    )
    .unwrap();
    let mut reg = Registry::builtin();
    let errors = reg.load_dir(&dir);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].contains("broken.toml"));
    assert_eq!(
        reg.detect(Some(Path::new("a.my")), b"").as_deref(),
        Some("mylang")
    );
    assert!(reg.is_user("mylang"));
    let t = tokens(&reg.get("mylang").unwrap(), "foo bar");
    assert_eq!(t[0], ["keyword:foo"]);
    std::fs::remove_dir_all(&dir).unwrap();
}

fn snap(text: &str) -> Snapshot {
    let v: Arc<Vec<u8>> = Arc::new(text.as_bytes().to_vec());
    let len = v.len() as u64;
    Snapshot::from_source_with_chunk(v, 0..len, 7, true)
}

/// 行頭の状態を先頭から逐次に求めたもの（オラクル）。
fn oracle(s: &Syntax, text: &str) -> Vec<(u64, LineState)> {
    let mut out = Vec::new();
    let mut st = LineState::default();
    let mut off = 0u64;
    for line in text.split_inclusive('\n') {
        out.push((off, st.clone()));
        let content = line.trim_end_matches('\n').trim_end_matches('\r');
        st = s.highlight_line(&st, content.as_bytes(), &mut Vec::new());
        off += line.len() as u64;
    }
    out
}

#[test]
fn index_states_match_sequential_scan() {
    let c = get("c");
    let text = "int a; /* open\nstill\n*/ int b = \"s\";\n// x\nchar *p = \"/*\";\n".repeat(30);
    let s = snap(&text);
    for block in [8, 50, 1000] {
        let mut idx = SyntaxIndex::with_block(c.clone(), block);
        while !idx.extend(&s, 37) {}
        for (off, st) in oracle(&c, &text) {
            assert_eq!(idx.state_at(&s, off), Some(st), "block {block} at {off}");
        }
        let (lines, exact) = idx.highlight_lines(&s, 0, s.len());
        assert!(exact);
        assert_eq!(lines.len(), 150);
    }
}

#[test]
fn provisional_colors_before_scanning() {
    let c = get("c");
    let text = "int x;\n".repeat(100_000);
    let s = snap(&text);
    let idx = SyntaxIndex::new(c.clone());
    // 読んでいない遠い位置では暫定の色（初期状態を仮定）
    let far = (text.len() - 7) as u64;
    assert_eq!(idx.state_at(&s, far), None);
    let (lines, exact) = idx.highlight_lines(&s, far, s.len());
    assert!(!exact);
    assert_eq!(c.token_name(lines[0].spans[0].token), "type");
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// 編集後に変わっていない先頭部分から読み直した結果が、全体を読み直した結果と一致する。
    #[test]
    fn incremental_rescan_matches_full_scan(
        base in proptest::collection::vec(
            prop_oneof![
                Just("int a;\n"), Just("/* c\n"), Just("*/\n"), Just("\"s\" // x\n"),
                Just("x = '/*';\n"), Just("\n"),
            ],
            1..40,
        ),
        edits in proptest::collection::vec((0usize..400, 0usize..6, prop_oneof![Just(""), Just("/*"), Just("*/"), Just("\n"), Just("\"")]), 1..6),
        block in 4u64..64,
    ) {
        let c = get("c");
        let text: String = base.concat();
        let mut s = snap(&text);
        let mut idx = SyntaxIndex::with_block(c.clone(), block);
        idx.extend(&s, u64::MAX);
        for (at, del, ins) in edits {
            let at = (at as u64).min(s.len());
            let del = (del as u64).min(s.len() - at);
            let edited = s.delete(at..at + del).insert(at, ins.as_bytes());
            let prefix = yy_syntax_common_prefix(&s, &edited);
            idx.truncate(prefix);
            idx.extend(&edited, u64::MAX);
            s = edited;
            let t = String::from_utf8(s.read(0..s.len())).unwrap();
            for (off, st) in oracle(&c, &t) {
                prop_assert_eq!(idx.state_at(&s, off), Some(st));
            }
        }
    }
}

/// 2 つの内容が先頭から一致している長さ（テスト用に単純に比べる）。
fn yy_syntax_common_prefix(a: &Snapshot, b: &Snapshot) -> u64 {
    let (x, y) = (a.read(0..a.len()), b.read(0..b.len()));
    x.iter().zip(y.iter()).take_while(|(p, q)| p == q).count() as u64
}
