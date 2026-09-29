//! 区切り文字形式（CSV / TSV など）の解析（04 章）。
//!
//! * [`Dialect`] … 区切り文字と引用符の有無（引用符ありなら RFC 4180 の規則）
//! * [`Scanner`] … バイト列を先頭から読んで、引用符の内外・フィールド番号・レコード数を追う
//!   状態機械（チャンクに分けて読める）
//! * [`split_line`] … 1 行をフィールドに分ける（表示用）
//! * [`RecordIndex`] … 一定間隔の位置での解析状態（任意の行の先頭が引用符の中かを
//!   その近くから読むだけで求められる。04 章 3.2）
//! * [`RecordReader`] / [`quote_field`] … フィールドの値の取り出しと書き出し（変換・列操作用）
//! * [`sniff`] … 方言の推定
//!
//! 引用符の扱いは寛容（lenient）: 引用符で始まらないフィールド中の `"` は普通の文字、
//! 閉じ引用符の後に区切り文字以外が続いた場合はそのまま同じフィールドとして続ける（Excel と同様）。

mod index;
mod reader;
mod sniff;

use std::ops::Range;

pub use index::{BLOCK, Point, RecordIndex, common_prefix};
pub use reader::{Record, RecordReader, quote_field, unquote, write_record};
pub use sniff::sniff;

/// 区切り文字と引用符。
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Dialect {
    delim: [u8; 4],
    delim_len: u8,
    /// 引用符（`Some(b'"')` なら RFC 4180 の規則。`None` なら引用符を解釈しない）
    pub quote: Option<u8>,
}

impl std::fmt::Debug for Dialect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Dialect({:?}, quote={:?})",
            self.name(),
            self.quote.map(char::from)
        )
    }
}

impl Dialect {
    /// 区切り文字（1〜4 バイト。改行・引用符を含まないこと）と引用符を指定して作る。
    pub fn new(delim: &[u8], quote: Option<u8>) -> Option<Dialect> {
        if delim.is_empty()
            || delim.len() > 4
            || delim
                .iter()
                .any(|&b| b == b'\n' || b == b'\r' || Some(b) == quote)
        {
            return None;
        }
        let mut d = [0u8; 4];
        d[..delim.len()].copy_from_slice(delim);
        Some(Dialect {
            delim: d,
            delim_len: delim.len() as u8,
            quote,
        })
    }

    pub fn csv() -> Dialect {
        Dialect::new(b",", Some(b'"')).unwrap()
    }

    pub fn tsv() -> Dialect {
        Dialect::new(b"\t", Some(b'"')).unwrap()
    }

    pub fn delimiter(&self) -> &[u8] {
        &self.delim[..self.delim_len as usize]
    }

    /// 表示用の名前（`,` `Tab` など）。
    pub fn name(&self) -> String {
        match self.delimiter() {
            b"," => "カンマ".into(),
            b"\t" => "タブ".into(),
            b";" => "セミコロン".into(),
            b"|" => "パイプ".into(),
            b" " => "空白".into(),
            d => String::from_utf8_lossy(d).into_owned(),
        }
    }

    /// 拡張子から既定の方言を決める（`csv` → カンマ、`tsv` `tab` → タブ）。
    pub fn for_extension(ext: &str) -> Option<Dialect> {
        match ext.to_ascii_lowercase().as_str() {
            "csv" => Some(Dialect::csv()),
            "tsv" | "tab" => Some(Dialect::tsv()),
            _ => None,
        }
    }

    /// `bytes[i..]` が区切り文字で始まるか。
    #[inline]
    fn delim_at(&self, bytes: &[u8], i: usize) -> bool {
        let d = self.delimiter();
        bytes.len() >= i + d.len() && &bytes[i..i + d.len()] == d
    }
}

/// 解析の状態。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum State {
    /// フィールドの先頭
    #[default]
    FieldStart,
    /// 引用符のないフィールドの中
    Unquoted,
    /// 引用符の中
    Quoted,
    /// 引用符の中で `"` を読んだ直後（閉じ引用符か `""` の 1 文字目）
    QuoteInQuoted,
}

/// 行の先頭での状態（その行がどのフィールドの途中から始まるか）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct LineState {
    /// 前の行から続く引用符の中で始まる（フィールド内の改行の後）
    pub in_quotes: bool,
    /// レコード内のフィールド番号（0 始まり）
    pub field: u32,
}

/// 状態機械。バイト列をチャンクに分けて与えられる。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Scanner {
    pub dialect: Dialect,
    pub state: State,
    /// レコード内のフィールド番号
    pub field: u32,
    /// 読み終えたレコードの数（引用符の外の LF の数）
    pub records: u64,
    /// 複数バイトの区切り文字が前のチャンクの末尾で途中まで一致している長さ
    partial: u8,
}

impl Scanner {
    pub fn new(dialect: Dialect) -> Scanner {
        Scanner {
            dialect,
            state: State::FieldStart,
            field: 0,
            records: 0,
            partial: 0,
        }
    }

    /// 行の先頭の状態から始める。
    pub fn at_line(dialect: Dialect, ls: LineState) -> Scanner {
        let mut s = Scanner::new(dialect);
        s.field = ls.field;
        if ls.in_quotes {
            s.state = State::Quoted;
        }
        s
    }

    /// 現在の位置が行の先頭だとしたときの行の状態。
    pub fn line_state(&self) -> LineState {
        LineState {
            in_quotes: self.state == State::Quoted,
            field: if self.state == State::Quoted {
                self.field
            } else {
                0
            },
        }
    }

    fn end_field(&mut self) {
        self.state = State::FieldStart;
        self.field += 1;
    }

    fn end_record(&mut self) {
        self.state = State::FieldStart;
        self.field = 0;
        self.records += 1;
    }

    /// `bytes` を読み進める。
    pub fn feed(&mut self, bytes: &[u8]) {
        let d = self.dialect;
        let delim = d.delimiter();
        let q = d.quote;
        let mut i = 0;
        // 前のチャンクから続く複数バイトの区切り文字
        if self.partial > 0 {
            let p = self.partial as usize;
            let need = delim.len() - p;
            let avail = bytes.len().min(need);
            if bytes[..avail] == delim[p..p + avail] {
                if avail == need {
                    self.partial = 0;
                    self.end_field();
                    i = need;
                } else {
                    self.partial += avail as u8;
                    return;
                }
            } else {
                // 区切り文字ではなかった: 1 バイト目は普通の文字として扱った状態に戻す
                self.partial = 0;
                if self.state == State::FieldStart || self.state == State::QuoteInQuoted {
                    self.state = State::Unquoted;
                }
            }
        }
        let n = bytes.len();
        while i < n {
            match self.state {
                State::Quoted => {
                    let qc = q.expect("quoted state requires a quote");
                    match memchr::memchr(qc, &bytes[i..]) {
                        Some(k) => {
                            i += k + 1;
                            self.state = State::QuoteInQuoted;
                        }
                        None => return,
                    }
                }
                State::Unquoted => match memchr::memchr2(delim[0], b'\n', &bytes[i..]) {
                    Some(k) => {
                        let p = i + k;
                        if bytes[p] == b'\n' {
                            self.end_record();
                            i = p + 1;
                        } else if d.delim_at(bytes, p) {
                            self.end_field();
                            i = p + delim.len();
                        } else if delim.len() > 1
                            && bytes.len() - p < delim.len()
                            && bytes[p..] == delim[..bytes.len() - p]
                        {
                            self.partial = (bytes.len() - p) as u8;
                            return;
                        } else {
                            i = p + 1;
                        }
                    }
                    None => return,
                },
                State::FieldStart | State::QuoteInQuoted => {
                    let b = bytes[i];
                    let was_quote_in = self.state == State::QuoteInQuoted;
                    if b == b'\n' {
                        self.end_record();
                        i += 1;
                    } else if Some(b) == q {
                        // 引用符の開始、または "" の 2 文字目
                        self.state = State::Quoted;
                        i += 1;
                    } else if b == delim[0] && d.delim_at(bytes, i) {
                        self.end_field();
                        i += delim.len();
                    } else if b == delim[0]
                        && delim.len() > 1
                        && bytes.len() - i < delim.len()
                        && bytes[i..] == delim[..bytes.len() - i]
                    {
                        self.partial = (bytes.len() - i) as u8;
                        return;
                    } else {
                        // 閉じ引用符の後の余計な文字（寛容に同じフィールドとして続ける）か、
                        // 引用符のないフィールドの始まり
                        let _ = was_quote_in;
                        self.state = State::Unquoted;
                    }
                }
            }
        }
    }
}

/// 1 行の中のフィールド。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cell {
    /// フィールド番号（レコード内、0 始まり）
    pub field: u32,
    /// 行内のフィールドの範囲（引用符を含む）
    pub range: Range<usize>,
    /// 直後の区切り文字の範囲（行の最後のフィールドなら `None`）
    pub delim: Option<Range<usize>>,
}

/// 1 行（改行を含まない）をフィールドに分ける。`start` は行の先頭の状態。
/// 戻り値の 2 つ目は次の行の先頭の状態。
pub fn split_line(line: &[u8], dialect: &Dialect, start: LineState) -> (Vec<Cell>, LineState) {
    let mut s = Scanner::at_line(*dialect, start);
    let mut cells = Vec::new();
    let mut field_start = 0;
    let delim = dialect.delimiter();
    let mut i = 0;
    let n = line.len();
    while i < n {
        let before = s.field;
        match s.state {
            State::Quoted => {
                let qc = dialect.quote.unwrap();
                match memchr::memchr(qc, &line[i..]) {
                    Some(k) => {
                        i += k + 1;
                        s.state = State::QuoteInQuoted;
                    }
                    None => i = n,
                }
                continue;
            }
            State::Unquoted => match memchr::memchr(delim[0], &line[i..]) {
                Some(k) => {
                    let p = i + k;
                    if dialect.delim_at(line, p) {
                        s.end_field();
                        cells.push(Cell {
                            field: before,
                            range: field_start..p,
                            delim: Some(p..p + delim.len()),
                        });
                        i = p + delim.len();
                        field_start = i;
                    } else {
                        i = p + 1;
                    }
                }
                None => i = n,
            },
            State::FieldStart | State::QuoteInQuoted => {
                let b = line[i];
                if Some(b) == dialect.quote {
                    s.state = State::Quoted;
                    i += 1;
                } else if dialect.delim_at(line, i) {
                    s.end_field();
                    cells.push(Cell {
                        field: before,
                        range: field_start..i,
                        delim: Some(i..i + delim.len()),
                    });
                    i += delim.len();
                    field_start = i;
                } else {
                    s.state = State::Unquoted;
                    i += 1;
                }
            }
        }
    }
    cells.push(Cell {
        field: s.field,
        range: field_start..n,
        delim: None,
    });
    let next = if s.state == State::Quoted {
        LineState {
            in_quotes: true,
            field: s.field,
        }
    } else {
        LineState::default()
    };
    (cells, next)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(line: &str, st: LineState) -> (Vec<(u32, &str, bool)>, LineState) {
        let (cells, next) = split_line(line.as_bytes(), &Dialect::csv(), st);
        (
            cells
                .iter()
                .map(|c| (c.field, &line[c.range.clone()], c.delim.is_some()))
                .collect(),
            next,
        )
    }

    #[test]
    fn splits_rfc4180_lines() {
        let (f, next) = fields(r#"a,"b,c","d""e",,"x"#, LineState::default());
        assert_eq!(
            f,
            vec![
                (0, "a", true),
                (1, "\"b,c\"", true),
                (2, "\"d\"\"e\"", true),
                (3, "", true),
                (4, "\"x", false)
            ]
        );
        // 引用符が閉じていないので次の行はフィールド 4 の続き
        assert_eq!(
            next,
            LineState {
                in_quotes: true,
                field: 4
            }
        );
        let (f, next) = fields(r#"y",z"#, next);
        assert_eq!(f, vec![(4, "y\"", true), (5, "z", false)]);
        assert_eq!(next, LineState::default());
        // 引用符で始まらないフィールドの " は普通の文字
        let (f, _) = fields(r#"ab"c,d"#, LineState::default());
        assert_eq!(f, vec![(0, "ab\"c", true), (1, "d", false)]);
        // 空行
        let (f, _) = fields("", LineState::default());
        assert_eq!(f, vec![(0, "", false)]);
    }

    #[test]
    fn multi_byte_delimiter() {
        let d = Dialect::new(b"||", None).unwrap();
        let (cells, _) = split_line(b"a||b|c||", &d, LineState::default());
        let got: Vec<_> = cells.iter().map(|c| c.range.clone()).collect();
        assert_eq!(got, vec![0..1, 3..6, 8..8]);
        // チャンクの境界で区切り文字が切れても同じ
        for cut in 0..=8 {
            let mut s = Scanner::new(d);
            s.feed(&b"a||b|c||"[..cut]);
            s.feed(&b"a||b|c||"[cut..]);
            assert_eq!(s.field, 2, "cut {cut}");
        }
    }

    #[test]
    fn scanner_counts_records_outside_quotes() {
        let text = b"a,b\n\"multi\nline\",c\n\"x\"\"\ny\"\nlast";
        for cut in 0..text.len() {
            let mut s = Scanner::new(Dialect::csv());
            s.feed(&text[..cut]);
            s.feed(&text[cut..]);
            assert_eq!(s.records, 3, "cut {cut}");
            assert_eq!(s.field, 0);
        }
    }
}
