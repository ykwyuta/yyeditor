//! フィールドの値の取り出しと書き出し。

use std::ops::Range;

use yy_buffer::Snapshot;

use crate::{Dialect, LineState, split_line};

/// ファイル中の表記（引用符付きなら引用符を含む）からフィールドの値を取り出す。
pub fn unquote(raw: &[u8], d: &Dialect) -> Vec<u8> {
    let Some(q) = d.quote else {
        return raw.to_vec();
    };
    if raw.first() != Some(&q) {
        return raw.to_vec();
    }
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 1;
    let mut closed = false;
    while i < raw.len() {
        let b = raw[i];
        if !closed && b == q {
            if raw.get(i + 1) == Some(&q) {
                out.push(q);
                i += 2;
                continue;
            }
            closed = true;
            i += 1;
            continue;
        }
        // 閉じ引用符の後の余計な文字もそのまま値に含める（寛容な解釈）
        out.push(b);
        i += 1;
    }
    out
}

/// 値をフィールドとして書き出す表記。区切り文字・引用符・改行を含む場合は引用符で囲む。
/// 引用符を使わない方言ではそのまま（区切り文字を含むと列がずれる）。
pub fn quote_field(value: &[u8], d: &Dialect) -> Vec<u8> {
    let Some(q) = d.quote else {
        return value.to_vec();
    };
    let delim = d.delimiter();
    let needs = value.windows(delim.len()).any(|w| w == delim)
        || value.iter().any(|&b| b == q || b == b'\n' || b == b'\r');
    if !needs {
        return value.to_vec();
    }
    let mut out = Vec::with_capacity(value.len() + 2);
    out.push(q);
    for &b in value {
        if b == q {
            out.push(q);
        }
        out.push(b);
    }
    out.push(q);
    out
}

/// 値の列を 1 レコードとして書き出す。空の値 1 つだけのレコードは空行と区別できるよう `""` にする。
pub fn write_record<V: AsRef<[u8]>>(
    values: &[V],
    d: &Dialect,
    terminator: &[u8],
    out: &mut Vec<u8>,
) {
    if let ([v], Some(q)) = (values, d.quote)
        && v.as_ref().is_empty()
    {
        out.extend_from_slice(&[q, q]);
    } else {
        for (i, v) in values.iter().enumerate() {
            if i > 0 {
                out.extend_from_slice(d.delimiter());
            }
            out.extend(quote_field(v.as_ref(), d));
        }
    }
    out.extend_from_slice(terminator);
}

/// 1 レコード。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Record {
    /// 各フィールドの表記（ファイル中のまま。引用符付きなら引用符を含む）
    pub fields: Vec<Vec<u8>>,
    /// レコードの終わりの改行（`\r\n` `\n`、最後のレコードで改行がなければ空）
    pub terminator: Vec<u8>,
    /// 文書内の範囲（改行を含む）
    pub range: Range<u64>,
}

/// スナップショットの範囲からレコードを順に読む。
pub struct RecordReader<'a> {
    snap: &'a Snapshot,
    d: Dialect,
    pos: u64,
    end: u64,
}

impl<'a> RecordReader<'a> {
    /// `start` はレコードの先頭であること。
    pub fn new(snap: &'a Snapshot, d: Dialect, start: u64, end: u64) -> RecordReader<'a> {
        RecordReader {
            snap,
            d,
            pos: start,
            end: end.min(snap.len()),
        }
    }

    /// 読んだ位置。
    pub fn position(&self) -> u64 {
        self.pos
    }
}

impl Iterator for RecordReader<'_> {
    type Item = Record;

    fn next(&mut self) -> Option<Record> {
        if self.pos >= self.end {
            return None;
        }
        let start = self.pos;
        let mut rec = Record::default();
        let mut state = LineState::default();
        loop {
            let nl = self.snap.find_next(self.pos..self.end, b'\n');
            let line_end = nl.unwrap_or(self.end);
            let line = self.snap.read(self.pos..line_end);
            let (cells, next) = split_line(&line, &self.d, state);
            for (i, c) in cells.iter().enumerate() {
                let bytes = &line[c.range.clone()];
                if i == 0 && state.in_quotes {
                    // 前の行から続くフィールド
                    let last = rec.fields.last_mut().expect("continued field");
                    last.extend_from_slice(bytes);
                } else {
                    rec.fields.push(bytes.to_vec());
                }
            }
            self.pos = nl.map_or(self.end, |n| n + 1);
            if next.in_quotes && nl.is_some() {
                // フィールド内の改行
                rec.fields.last_mut().unwrap().push(b'\n');
                state = next;
                continue;
            }
            if nl.is_some() {
                // レコードの終わりの改行（CRLF の CR は最後のフィールドから外す）
                let last = rec.fields.last_mut().unwrap();
                if last.last() == Some(&b'\r') {
                    last.pop();
                    rec.terminator = b"\r\n".to_vec();
                } else {
                    rec.terminator = b"\n".to_vec();
                }
            }
            break;
        }
        rec.range = start..self.pos;
        Some(rec)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn snap(text: &str) -> Snapshot {
        let v: Arc<Vec<u8>> = Arc::new(text.as_bytes().to_vec());
        let len = v.len() as u64;
        Snapshot::from_source_with_chunk(v, 0..len, 5, true)
    }

    #[test]
    fn quoting_round_trip() {
        let d = Dialect::csv();
        for v in ["plain", "a,b", "say \"hi\"", "multi\r\nline", "", " sp "] {
            let q = quote_field(v.as_bytes(), &d);
            assert_eq!(
                unquote(&q, &d),
                v.as_bytes(),
                "{v:?} -> {:?}",
                String::from_utf8_lossy(&q)
            );
        }
        assert_eq!(quote_field(b"a,b", &d), b"\"a,b\"");
        assert_eq!(unquote(b"\"ab\"cd", &d), b"abcd");
    }

    #[test]
    fn reads_records_with_embedded_newlines() {
        let text = "a,b\r\n\"x\r\ny\",\"q\"\"\"\r\nlast,";
        let s = snap(text);
        let recs: Vec<Record> = RecordReader::new(&s, Dialect::csv(), 0, s.len()).collect();
        assert_eq!(recs.len(), 3);
        assert_eq!(recs[0].fields, vec![b"a".to_vec(), b"b".to_vec()]);
        assert_eq!(recs[0].terminator, b"\r\n");
        assert_eq!(
            recs[1].fields,
            vec![b"\"x\r\ny\"".to_vec(), b"\"q\"\"\"".to_vec()]
        );
        assert_eq!(recs[2].fields, vec![b"last".to_vec(), b"".to_vec()]);
        assert_eq!(recs[2].terminator, b"");
        assert_eq!(recs[2].range.end, s.len());
    }
}
