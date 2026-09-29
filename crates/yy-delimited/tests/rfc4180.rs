//! RFC 4180 の規定例と、`csv` クレートとの差分テスト（08 章 2.1）。

use std::sync::Arc;

use proptest::prelude::*;
use yy_buffer::Snapshot;
use yy_delimited::{Dialect, LineState, RecordIndex, RecordReader, Scanner, unquote, write_record};

fn snap(bytes: &[u8], chunk: u32) -> Snapshot {
    let v: Arc<Vec<u8>> = Arc::new(bytes.to_vec());
    let len = v.len() as u64;
    Snapshot::from_source_with_chunk(v, 0..len, chunk, true)
}

/// レコードごとのフィールドの値。
fn values(bytes: &[u8], d: Dialect) -> Vec<Vec<String>> {
    let s = snap(bytes, 7);
    RecordReader::new(&s, d, 0, s.len())
        .map(|r| {
            r.fields
                .iter()
                .map(|f| String::from_utf8(unquote(f, &d)).unwrap())
                .collect()
        })
        .collect()
}

fn rows(v: &[&[&str]]) -> Vec<Vec<String>> {
    v.iter()
        .map(|r| r.iter().map(|s| s.to_string()).collect())
        .collect()
}

#[test]
fn rfc4180_section2_examples() {
    let d = Dialect::csv();
    // 1. 各レコードは CRLF で区切る
    assert_eq!(
        values(b"aaa,bbb,ccc\r\nzzz,yyy,xxx\r\n", d),
        rows(&[&["aaa", "bbb", "ccc"], &["zzz", "yyy", "xxx"]])
    );
    // 2. 最後のレコードの改行はなくてもよい
    assert_eq!(
        values(b"aaa,bbb,ccc\r\nzzz,yyy,xxx", d),
        rows(&[&["aaa", "bbb", "ccc"], &["zzz", "yyy", "xxx"]])
    );
    // 4. 空白はフィールドの一部。最後のフィールドの後に区切り文字を置かない
    assert_eq!(
        values(b"aaa , bbb,ccc", d),
        rows(&[&["aaa ", " bbb", "ccc"]])
    );
    // 5. フィールドは引用符で囲んでもよい
    assert_eq!(
        values(b"\"aaa\",\"bbb\",\"ccc\"\r\nzzz,yyy,xxx", d),
        rows(&[&["aaa", "bbb", "ccc"], &["zzz", "yyy", "xxx"]])
    );
    // 6. 改行・引用符・カンマを含むフィールドは引用符で囲む
    assert_eq!(
        values(b"\"aaa\",\"b\r\nbb\",\"ccc\"\r\nzzz,yyy,xxx", d),
        rows(&[&["aaa", "b\r\nbb", "ccc"], &["zzz", "yyy", "xxx"]])
    );
    // 7. 引用符は 2 つ重ねる
    assert_eq!(
        values(b"\"aaa\",\"b\"\"bb\",\"ccc\"", d),
        rows(&[&["aaa", "b\"bb", "ccc"]])
    );
    // 空のフィールド・空のレコード
    assert_eq!(values(b",,\r\n\"\"\r\n", d), rows(&[&["", "", ""], &[""]]));
}

fn value_strategy() -> impl Strategy<Value = String> {
    prop::collection::vec(
        prop_oneof![
            Just("a"),
            Just("あ"),
            Just(","),
            Just("\""),
            Just("\r\n"),
            Just("\n"),
            Just(" "),
            Just("\t"),
        ],
        0..6,
    )
    .prop_map(|v| v.concat())
}

fn table_strategy() -> impl Strategy<Value = Vec<Vec<String>>> {
    prop::collection::vec(prop::collection::vec(value_strategy(), 1..5), 1..8)
}

/// `csv` クレートで書き出す（必要なときだけ引用符で囲む）。
fn write_with_csv_crate(table: &[Vec<String>], delim: u8) -> Vec<u8> {
    let mut w = csv::WriterBuilder::new()
        .delimiter(delim)
        .flexible(true)
        .terminator(csv::Terminator::CRLF)
        .from_writer(Vec::new());
    for r in table {
        w.write_record(r).unwrap();
    }
    w.into_inner().unwrap()
}

/// 1 フィールドだけの空のレコードは `csv` クレートが `""` と書き、空行とは区別される。
fn normalize(table: &[Vec<String>]) -> Vec<Vec<String>> {
    table.to_vec()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(500))]

    /// `csv` クレートが書いた CSV / TSV を同じ値として読める。
    #[test]
    fn reads_what_csv_crate_writes(table in table_strategy(), tab in any::<bool>()) {
        let (delim, d) = if tab { (b'\t', Dialect::tsv()) } else { (b',', Dialect::csv()) };
        let bytes = write_with_csv_crate(&table, delim);
        prop_assert_eq!(values(&bytes, d), normalize(&table));
    }

    /// 自分で書き出した CSV を `csv` クレートが同じ値として読める。
    #[test]
    fn csv_crate_reads_what_we_write(table in table_strategy()) {
        let d = Dialect::csv();
        let mut bytes = Vec::new();
        for r in &table {
            write_record(r, &d, b"\r\n", &mut bytes);
        }
        let mut rd = csv::ReaderBuilder::new()
            .has_headers(false)
            .flexible(true)
            .from_reader(&bytes[..]);
        let got: Vec<Vec<String>> = rd
            .records()
            .map(|r| r.unwrap().iter().map(|s| s.to_string()).collect())
            .collect();
        prop_assert_eq!(got, table);
    }

    /// インデックスから求めた各行の先頭の状態が、先頭から逐次に読んだ結果と一致する。
    #[test]
    fn index_matches_sequential_scan(
        table in table_strategy(),
        block in 3u64..40,
        chunk in 5u32..30,
    ) {
        let d = Dialect::csv();
        let bytes = write_with_csv_crate(&table, b',');
        let s = snap(&bytes, chunk);
        let mut idx = RecordIndex::with_block(d, block);
        while !idx.extend(&s, 11) {}
        let mut sc = Scanner::new(d);
        let mut expect = vec![(0u64, LineState::default())];
        for (i, &b) in bytes.iter().enumerate() {
            sc.feed(&[b]);
            if b == b'\n' {
                expect.push((i as u64 + 1, sc.line_state()));
            }
        }
        for (off, st) in expect {
            prop_assert_eq!(idx.line_state_at(&s, off), st);
        }
        prop_assert_eq!(idx.record_count(&s), Some(table.len() as u64));
    }
}
