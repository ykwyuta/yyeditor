//! ウィンドウ検索の結果が、全文を 1 度に検索した結果と一致することの検証（08 章 2.1）。

use std::sync::Arc;

use proptest::prelude::*;
use regex_automata::meta::Regex;
use regex_automata::util::syntax;
use regex_automata::{Anchored, Input};
use yy_buffer::Snapshot;
use yy_search::{Query, Replacement, Searcher, collect_edits, rewrite};

const PATTERNS: &[&str] = &[
    "a",
    "ab",
    "a+",
    "a*",
    "^a",
    "b$",
    r"\bab\b",
    "[ab]{2,3}",
    "(a)(b)?",
    "あ+",
    ".",
    r"a\r?\n",
    "(?s)a.b",
    "^$",
    r"b\s*a",
];

fn regex(q: &Query) -> Regex {
    let pat = if q.regex {
        q.pattern.clone()
    } else {
        regex_syntax::escape(&q.pattern)
    };
    Regex::builder()
        .syntax(
            syntax::Config::new()
                .case_insensitive(!q.case_sensitive)
                .multi_line(true)
                .crlf(true),
        )
        .build(&pat)
        .unwrap()
}

/// 小さなピースに分けたスナップショット（ピースの境界をまたぐマッチを確かめる）。
fn snapshot(text: &str, chunk: u32) -> Snapshot {
    let v: Arc<Vec<u8>> = Arc::new(text.as_bytes().to_vec());
    let len = v.len() as u64;
    Snapshot::from_source_with_chunk(v, 0..len, chunk, true)
}

fn query(p: &str) -> Query {
    Query {
        pattern: p.to_owned(),
        regex: true,
        case_sensitive: true,
        whole_word: false,
    }
}

fn text_strategy() -> impl Strategy<Value = String> {
    prop::collection::vec(
        prop_oneof![
            Just("a"),
            Just("b"),
            Just(" "),
            Just("\n"),
            Just("\r\n"),
            Just("あ"),
            Just("ab"),
        ],
        0..120,
    )
    .prop_map(|v| v.concat())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]

    #[test]
    fn window_search_matches_whole_text(
        text in text_strategy(),
        pat in 0..PATTERNS.len(),
        window in 20u64..80,
        chunk in 5u32..20,
        from_frac in 0.0f64..1.0,
    ) {
        let q = query(PATTERNS[pat]);
        let re = regex(&q);
        let bytes = text.as_bytes();
        let len = bytes.len() as u64;
        let snap = snapshot(&text, chunk);
        let s = Searcher::new(&q).unwrap();
        // 最大長が無制限のパターンはウィンドウを小さくできないので、上限のあるパターンだけ
        let s = if s.max_match_len() < 1 << 20 { s.with_window(window) } else { s };

        // すべてのマッチ
        let expect: Vec<_> = re
            .find_iter(bytes)
            .map(|m| m.start() as u64..m.end() as u64)
            .collect();
        prop_assert_eq!(&s.matches_in(&snap, 0..len, usize::MAX), &expect);
        prop_assert_eq!(s.count(&snap, 0..len, u64::MAX, &mut |_| true).unwrap(), expect.len() as u64);

        // 次を検索
        let from = (len as f64 * from_frac) as u64;
        let next = re
            .search(&Input::new(bytes).range(from as usize..))
            .map(|m| m.start() as u64..m.end() as u64);
        prop_assert_eq!(s.find_next(&snap, 0..len, from, &mut |_| true).unwrap(), next);

        // 前を検索: 開始位置が from より前で最大のマッチ
        let prev = (0..from as usize).rev().find_map(|st| {
            re.search(&Input::new(bytes).range(st..).anchored(Anchored::Yes))
                .map(|m| m.start() as u64..m.end() as u64)
        });
        prop_assert_eq!(s.find_prev(&snap, 0..len, from, &mut |_| true).unwrap(), prev);
    }

    #[test]
    fn rewrite_matches_edits(
        text in text_strategy(),
        pat in 0..PATTERNS.len(),
        window in 20u64..80,
    ) {
        let q = query(PATTERNS[pat]);
        let snap = snapshot(&text, 7);
        let len = snap.len();
        let s = Searcher::new(&q).unwrap();
        let small = Searcher::new(&q).unwrap();
        let small = if small.max_match_len() < 1 << 20 { small.with_window(window) } else { small };
        let repl = Replacement::parse("<\\U$0\\E>", &s).unwrap();
        let mut out = Vec::new();
        let n = rewrite(&small, &snap, 0..len, &repl, &mut out, &mut |_| true).unwrap();
        let edits = collect_edits(&s, &snap, 0..len, &repl, usize::MAX, &mut |_| true)
            .unwrap()
            .unwrap();
        prop_assert_eq!(n, edits.len() as u64);
        // 変更を後ろから適用した結果と一致
        let mut expect = text.as_bytes().to_vec();
        for (r, b) in edits.iter().rev() {
            expect.splice(r.start as usize..r.end as usize, b.iter().copied());
        }
        prop_assert_eq!(out, expect);
    }
}

#[test]
fn literal_and_options() {
    let text = "Foo foo FOO food (a.b) a+b";
    let snap = snapshot(text, 5);
    let len = snap.len();
    let find = |q: Query| Searcher::new(&q).unwrap().matches_in(&snap, 0..len, 100);
    let mut q = Query {
        pattern: "foo".into(),
        ..Query::default()
    };
    assert_eq!(find(q.clone()), vec![0..3, 4..7, 8..11, 12..15]);
    q.case_sensitive = true;
    assert_eq!(find(q.clone()), vec![4..7, 12..15]);
    q.whole_word = true;
    assert_eq!(find(q.clone()), vec![4..7]);
    // 文字列そのもの（正規表現の記号を含む）
    let q = Query {
        pattern: "(a.b)".into(),
        ..Query::default()
    };
    assert_eq!(find(q), vec![17..22]);
    // 単語単位でも記号を含むパターンを探せる
    let q = Query {
        pattern: "a+b".into(),
        whole_word: true,
        ..Query::default()
    };
    assert_eq!(find(q), vec![23..26]);
    // 誤り
    assert!(Searcher::new(&Query::default()).is_err());
    assert!(Searcher::new(&query("(")).is_err());
}

#[test]
fn line_anchors_with_crlf() {
    let text = "abc\r\ndef\r\n";
    let snap = snapshot(text, 5);
    let s = Searcher::new(&query("^d.*$")).unwrap();
    assert_eq!(s.matches_in(&snap, 0..snap.len(), 10), vec![5..8]);
}

#[test]
fn replacement_templates() {
    let q = query(r"(?<first>\w+) (\w+)");
    let s = Searcher::new(&q).unwrap();
    let snap = snapshot("hello world", 5);
    let run = |t: &str| {
        let r = Replacement::parse(t, &s).unwrap();
        let mut out = Vec::new();
        rewrite(&s, &snap, 0..snap.len(), &r, &mut out, &mut |_| true).unwrap();
        String::from_utf8(out).unwrap()
    };
    assert_eq!(run("$2 $1"), "world hello");
    assert_eq!(run("${first}-$$-\\2"), "hello-$-world");
    assert_eq!(run("\\U$1\\E \\u$2"), "HELLO World");
    assert_eq!(run("\\L\\U$1\\E!"), "HELLO!");
    assert_eq!(run("a\\tb\\nc"), "a\tb\nc");
    assert!(Replacement::parse("$3", &s).is_err());
    assert!(Replacement::parse("${nope}", &s).is_err());
    assert_eq!(
        Replacement::literal("$1\\n").as_literal(),
        Some(&b"$1\\n"[..])
    );
}

#[test]
fn long_matches_across_many_windows() {
    // 1 MiB を超えるテキストでもウィンドウの境界をまたいで見つかる
    let mut text = "x".repeat((9 << 20) - 3);
    text.push_str("needle");
    text.push_str(&"y".repeat(100));
    let snap = snapshot(&text, 1 << 20);
    let s = Searcher::new(&query("needle")).unwrap();
    assert_eq!(
        s.find_next(&snap, 0..snap.len(), 0, &mut |_| true).unwrap(),
        Some((9 << 20) - 3..(9 << 20) + 3)
    );
    assert_eq!(
        s.find_prev(&snap, 0..snap.len(), snap.len(), &mut |_| true)
            .unwrap(),
        Some((9 << 20) - 3..(9 << 20) + 3)
    );
    // 中止
    let mut calls = 0;
    let s = Searcher::new(&query("zzz")).unwrap().with_window(1 << 16);
    let r = s.find_next(&snap, 0..snap.len(), 0, &mut |_| {
        calls += 1;
        calls < 3
    });
    assert!(r.is_err());
}
