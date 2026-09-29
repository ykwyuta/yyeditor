//! 方言の推定（04 章 6）。
//!
//! 候補の区切り文字ごとに先頭のレコードのフィールド数を数え、フィールド数がそろっている
//! （最も多いフィールド数のレコードの割合が高い）区切り文字を選ぶ。

use std::collections::HashMap;

use crate::{Dialect, LineState, split_line};

const CANDIDATES: [&[u8]; 4] = [b",", b"\t", b";", b"|"];
/// 調べるレコードの数
const MAX_RECORDS: usize = 200;

/// `sample`（ファイルの先頭）から方言を推定する。区切り文字形式らしくなければ `None`。
pub fn sniff(sample: &[u8]) -> Option<Dialect> {
    // 最後の行は途中で切れているかもしれないので使わない
    let body = match sample.iter().rposition(|&b| b == b'\n') {
        Some(p) => &sample[..p],
        None => sample,
    };
    let mut best: Option<(f64, Dialect)> = None;
    for delim in CANDIDATES {
        let d = Dialect::new(delim, Some(b'"')).unwrap();
        let counts = field_counts(body, &d);
        if counts.len() < 2 {
            continue;
        }
        let mut freq: HashMap<u32, usize> = HashMap::new();
        for &c in &counts {
            *freq.entry(c).or_default() += 1;
        }
        let (&mode, &n) = freq.iter().max_by_key(|(k, v)| (**v, **k)).unwrap();
        if mode < 2 {
            continue;
        }
        let consistency = n as f64 / counts.len() as f64;
        if consistency < 0.8 {
            continue;
        }
        // フィールド数が多く、そろっているものほど高い
        let score = consistency * (mode as f64).ln_1p();
        if best.as_ref().is_none_or(|(s, _)| score > *s) {
            best = Some((score, d));
        }
    }
    best.map(|(_, d)| d)
}

/// 各レコードのフィールド数。
fn field_counts(body: &[u8], d: &Dialect) -> Vec<u32> {
    let mut out = Vec::new();
    let mut state = LineState::default();
    let mut fields = 0u32;
    for line in body.split(|&b| b == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let (cells, next) = split_line(line, d, state);
        fields = cells.last().map_or(fields, |c| c.field + 1);
        if !next.in_quotes {
            if !line.is_empty() || state.in_quotes {
                out.push(fields);
            }
            if out.len() >= MAX_RECORDS {
                break;
            }
        }
        state = next;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_common_dialects() {
        let csv = b"id,name,note\n1,\"a, b\",x\n2,c,\"multi\nline\"\n3,d,e\n";
        assert_eq!(sniff(csv), Some(Dialect::csv()));
        let tsv = b"id\tname\tnote\n1\ta, b\tx\n2\tc\ty\n";
        assert_eq!(sniff(tsv), Some(Dialect::tsv()));
        let semi = b"a;b;c\n1;2;3\n4;5;6\n";
        assert_eq!(
            sniff(semi).map(|d| d.delimiter().to_vec()),
            Some(b";".to_vec())
        );
        assert_eq!(sniff(b"just some prose.\nanother line, here.\n"), None);
    }
}
