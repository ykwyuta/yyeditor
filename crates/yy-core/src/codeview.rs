//! コード値の表示・編集モード。
//!
//! 16 進数（バイナリ）編集と同じ形で、文書の文字を 1 行 8 文字ずつ、左に各文字の Unicode の
//! 符号位置（コード値）、右に文字を並べて表示する。行は文書の行（改行）の中を 8 文字ずつに
//! 区切ったもので、行の先頭は近くの改行から求めるので、行の索引がなくても任意の位置を表示できる。
//!
//! ```text
//!      1  3042   3044   3046   0041    0042   000D   000A          あいうAB..
//! ```
//!
//! 読み込み時に不正だったバイト（エスケープ文字）と UTF-8 として不正なバイトは `\xNN` と表示する。

use std::ops::Range;

use yy_buffer::Snapshot;

use crate::hex::{CellMark, Pane, RowText};

/// 1 行の文字数。
pub const ROW_CHARS: usize = 8;
/// コード値の欄の 1 文字分の桁数（最大 6 桁の値と空白）。
const CODE_WIDTH: usize = 7;
/// 長い行の区切り。この倍数の位置を含む文字でも表示の行を改める（行の先頭を求めるときに
/// 改行を探すのは、この倍数の位置からで済む）。
const LINE_WINDOW: u64 = 64 * 1024;
/// コード値の最大の桁数。
pub const MAX_DIGITS: u8 = 6;

/// 表示の 1 文字（または不正な 1 バイト）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    /// 文書内の位置
    pub start: u64,
    /// バイト数
    pub len: u8,
    pub kind: CellKind,
}

impl Cell {
    pub fn end(&self) -> u64 {
        self.start + self.len as u64
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellKind {
    Char(char),
    /// 読み込み時に不正だったバイト（エスケープ文字）、または UTF-8 として不正なバイト
    Byte(u8),
}

/// 表示の 1 行。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeRow {
    pub start: u64,
    pub cells: Vec<Cell>,
    /// 文書の行の先頭の行なら、その行番号（1 始まり）
    pub line: Option<u64>,
}

impl CodeRow {
    pub fn end(&self) -> u64 {
        self.cells.last().map_or(self.start, |c| c.end())
    }

    /// 行が終わっている（8 文字ちょうど、または改行で終わる）か。
    fn closed(&self) -> bool {
        self.cells.len() == ROW_CHARS
            || matches!(self.cells.last(), Some(c) if c.kind == CellKind::Char('\n'))
    }
}

/// `bytes`（文書の位置 `base` から）の文字。`complete` でなければ、末尾の読み切れていない
/// 多バイト文字の断片は返さない。
fn decode_cells(bytes: &[u8], base: u64, complete: bool) -> Vec<Cell> {
    let mut cells = Vec::new();
    let mut pos = base;
    for chunk in bytes.utf8_chunks() {
        for c in chunk.valid().chars() {
            let len = c.len_utf8() as u8;
            let kind = match yy_encoding::unescape_char(c) {
                Some(b) => CellKind::Byte(b),
                None => CellKind::Char(c),
            };
            cells.push(Cell {
                start: pos,
                len,
                kind,
            });
            pos += len as u64;
        }
        for &b in chunk.invalid() {
            cells.push(Cell {
                start: pos,
                len: 1,
                kind: CellKind::Byte(b),
            });
            pos += 1;
        }
    }
    if !complete {
        // 末尾の 3 バイト以内の不正なバイトは、続きを読めば文字になるかもしれない
        let end = base + bytes.len() as u64;
        while let Some(c) = cells.last() {
            if matches!(c.kind, CellKind::Byte(_)) && c.len == 1 && end - c.start <= 3 {
                cells.pop();
            } else {
                break;
            }
        }
    }
    cells
}

/// `pos` から読んだ文字（`max_bytes` バイトまで。文字の途中では切らない）。
fn cells_from(snap: &Snapshot, pos: u64, max_bytes: u64) -> Vec<Cell> {
    let len = snap.len();
    let end = (pos + max_bytes).min(len);
    let bytes = snap.read(pos..end);
    decode_cells(&bytes, pos, end == len)
}

/// 文字 `cells` を行に分ける。`at_end` なら `cells` は文書の終わりまでで、終わりの位置
/// （カーソルを置く場所）を表す空の行も必要なら加える。
fn split_rows(cells: Vec<Cell>, start: u64, at_end: bool) -> Vec<CodeRow> {
    let mut rows = Vec::new();
    let mut cur = CodeRow {
        start,
        cells: Vec::new(),
        line: None,
    };
    for c in cells {
        // 長い行の区切り（LINE_WINDOW の倍数の位置を含む文字）の前で改める
        let boundary = c.start.div_ceil(LINE_WINDOW) * LINE_WINDOW;
        if !cur.cells.is_empty() && boundary < c.end() {
            let next = c.start;
            rows.push(std::mem::replace(
                &mut cur,
                CodeRow {
                    start: next,
                    cells: Vec::new(),
                    line: None,
                },
            ));
        }
        cur.cells.push(c);
        if cur.closed() {
            let next = c.end();
            rows.push(std::mem::replace(
                &mut cur,
                CodeRow {
                    start: next,
                    cells: Vec::new(),
                    line: None,
                },
            ));
        }
    }
    if !cur.cells.is_empty() || (at_end && rows.last().is_none_or(|r| r.closed())) {
        rows.push(cur);
    }
    rows
}

/// 文字の境界に合わせる（`pos` が多バイト文字の途中なら、その文字の先頭）。
pub fn align(snap: &Snapshot, pos: u64) -> u64 {
    let pos = pos.min(snap.len());
    let from = pos.saturating_sub(4);
    let cells = cells_from(snap, from, pos - from + 4);
    // 前の文字の途中から読み始めたかもしれないので、先頭の不正なバイトは信用しない
    cells
        .iter()
        .rev()
        .find(|c| c.start <= pos && !(matches!(c.kind, CellKind::Byte(_)) && c.len == 1))
        .filter(|c| pos < c.end())
        .map_or(pos, |c| c.start)
}

/// 位置 `pos` を含む表示の行の先頭。
pub fn row_start_at(snap: &Snapshot, pos: u64) -> u64 {
    let len = snap.len();
    let pos = pos.min(len);
    let window = pos / LINE_WINDOW * LINE_WINDOW;
    let line = match snap.find_prev(window..pos, b'\n') {
        Some(p) => p + 1,
        None if window == 0 => 0,
        None => align(snap, window),
    };
    let cells = cells_from(snap, line, pos - line + 4);
    let at_end = pos + 4 >= len;
    let rows = split_rows(cells, line, at_end);
    rows.iter()
        .rev()
        .find(|r| r.start <= pos)
        .map_or(line, |r| r.start)
}

/// 位置 `top`（表示の行の先頭）から `count` 行。文書の行の先頭の行には行番号を付ける。
pub fn rows_from(snap: &Snapshot, top: u64, count: usize) -> Vec<CodeRow> {
    let len = snap.len();
    let max = (count * ROW_CHARS * 4) as u64 + 4;
    let cells = cells_from(snap, top, max);
    let at_end = top + max >= len;
    let mut rows = split_rows(cells, top, at_end);
    rows.truncate(count);
    for r in &mut rows {
        let line_head = r.start == 0 || snap.byte_at(r.start - 1) == Some(b'\n');
        if line_head {
            r.line = Some(snap.line_of_offset(r.start).line + 1);
        }
    }
    rows
}

/// 次の表示の行の先頭（`top` が最後の行なら `None`）。
pub fn next_row(snap: &Snapshot, top: u64) -> Option<u64> {
    let rows = rows_from(snap, top, 2);
    rows.get(1).map(|r| r.start)
}

/// 前の表示の行の先頭（`top` が最初の行なら `None`）。
pub fn prev_row(snap: &Snapshot, top: u64) -> Option<u64> {
    (top > 0).then(|| row_start_at(snap, top - 1))
}

/// 位置 `pos` の文字の次の文字の位置（文書の終わりならそのまま）。
pub fn next_char(snap: &Snapshot, pos: u64) -> u64 {
    cells_from(snap, pos, 4).first().map_or(pos, |c| c.end())
}

/// 位置 `pos` の前の文字の位置。
pub fn prev_char(snap: &Snapshot, pos: u64) -> u64 {
    if pos == 0 {
        return 0;
    }
    align(snap, pos - 1)
}

/// 行 `rows` の中のカーソル `caret` の位置（行, 行内の文字の番号）。文書の終わり `len` は
/// 最後の文字の次。
pub fn locate(rows: &[CodeRow], caret: u64, len: u64) -> Option<(usize, usize)> {
    for (ri, r) in rows.iter().enumerate() {
        if let Some(i) = r.cells.iter().position(|c| c.start == caret) {
            return Some((ri, i));
        }
        if caret == len && r.end() == len && (r.cells.is_empty() || !r.closed()) {
            return Some((ri, r.cells.len()));
        }
    }
    None
}

/// コード値の表記（`3042`・`1F600`・`\x82`）。
pub fn code_text(kind: CellKind) -> String {
    match kind {
        CellKind::Char(c) => format!("{:04X}", c as u32),
        CellKind::Byte(b) => format!("\\x{b:02X}"),
    }
}

/// コード値の並び（コピー用。`3042 3044`）。
pub fn codes_text(cells: &[Cell]) -> String {
    cells
        .iter()
        .map(|c| code_text(c.kind))
        .collect::<Vec<_>>()
        .join(" ")
}

/// コード値の並び（`3042 3044`・`U+3042`・`0x3042`・`あ`・`\u{1F600}`。空白・カンマで
/// 区切る）を文字列にする。読めなければ `None`。
pub fn parse_codes(text: &str) -> Option<String> {
    let mut out = String::new();
    for tok in text.split(|c: char| c.is_whitespace() || c == ',' || c == ';') {
        if tok.is_empty() {
            continue;
        }
        let t = tok
            .strip_prefix("U+")
            .or_else(|| tok.strip_prefix("u+"))
            .or_else(|| tok.strip_prefix("0x"))
            .or_else(|| tok.strip_prefix("0X"))
            .or_else(|| tok.strip_prefix("\\u"))
            .unwrap_or(tok);
        let t = t
            .strip_prefix('{')
            .and_then(|t| t.strip_suffix('}'))
            .unwrap_or(t);
        if t.is_empty() || t.len() > MAX_DIGITS as usize {
            return None;
        }
        let v = u32::from_str_radix(t, 16).ok()?;
        out.push(char::from_u32(v)?);
    }
    (!out.is_empty()).then_some(out)
}

/// 入力中のコード値に数字 `digit` を加えた値。範囲を超えるなら `None`。
pub fn push_digit(entry: Option<(u32, u8)>, digit: u32) -> Option<(u32, u8)> {
    let (v, n) = entry.unwrap_or((0, 0));
    let v = v.checked_mul(16)? + digit;
    (n < MAX_DIGITS && v <= 0x10FFFF).then_some((v, n + 1))
}

/// コード値の表示の桁の配置（等幅フォントの桁）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CodeLayout {
    /// 行番号の桁数
    pub digits: usize,
}

impl CodeLayout {
    /// 文書の行数（の見積もり）`lines` に合わせた行番号の桁数（6 桁以上）。
    pub fn for_lines(lines: u64) -> CodeLayout {
        CodeLayout {
            digits: lines.max(1).to_string().len().max(6),
        }
    }

    /// 行内の `i` 番目の文字のコード値の先頭の桁。4 文字ごとに空白を 1 つ多く空ける。
    pub fn code_col(&self, i: usize) -> usize {
        self.digits + 2 + i * CODE_WIDTH + usize::from(i >= ROW_CHARS / 2)
    }

    /// 行内の `i` 番目の文字の文字の欄の桁（1 文字 2 桁）。
    pub fn char_col(&self, i: usize) -> usize {
        self.code_col(ROW_CHARS) + 1 + i * 2
    }

    /// 1 行の桁数。
    pub fn width(&self) -> usize {
        self.char_col(ROW_CHARS)
    }

    /// 桁 `col` の位置にあるもの（欄, 行内の文字の番号）。行番号の欄なら `None`。
    pub fn hit(&self, col: usize) -> Option<(Pane, usize)> {
        if col < self.code_col(0) {
            return None;
        }
        if col >= self.char_col(0) - 1 {
            let i = col.saturating_sub(self.char_col(0)) / 2;
            return Some((Pane::Ascii, i.min(ROW_CHARS - 1)));
        }
        let i = (0..ROW_CHARS)
            .rev()
            .find(|&i| col >= self.code_col(i))
            .unwrap_or(0);
        Some((Pane::Hex, i))
    }

    /// 1 行の表示テキストと色を変える範囲（UTF-16 の位置）。`entry` は入力中のコード値
    /// （行内の文字の番号, 入力した桁）で、その文字のコード値の代わりに表示する。
    pub fn format_row(
        &self,
        row: &CodeRow,
        entry: Option<(usize, &str)>,
        ambiguous_wide: bool,
    ) -> RowText {
        let mut text = match row.line {
            Some(n) => format!("{n:>width$}  ", width = self.digits),
            None => " ".repeat(self.digits + 2),
        };
        let mut marks = Vec::new();
        // コード値の欄（ASCII だけなので、桁と UTF-16 の位置は同じ）
        let n = row.cells.len().max(entry.map_or(0, |e| e.0 + 1));
        for i in 0..n {
            while text.len() < self.code_col(i) {
                text.push(' ');
            }
            let code = match (entry, row.cells.get(i)) {
                (Some((ei, s)), _) if ei == i => s.to_owned(),
                (_, Some(c)) => {
                    if matches!(c.kind, CellKind::Byte(_)) {
                        marks.push((text.len()..text.len() + 4, CellMark::Invalid));
                    }
                    code_text(c.kind)
                }
                (_, None) => String::new(),
            };
            text += &code;
        }
        while text.len() < self.char_col(0) {
            text.push(' ');
        }
        // 文字の欄（1 文字 2 桁。半角の文字は後ろに空白）
        let mut u16pos = text.len();
        for c in &row.cells {
            let (s, width, mark) = match c.kind {
                CellKind::Byte(_) => (".".to_owned(), 1, Some(CellMark::Invalid)),
                CellKind::Char(ch) => {
                    let mut buf = [0u8; 4];
                    match crate::hex::display_char(ch.encode_utf8(&mut buf), ambiguous_wide) {
                        Some((s, w)) => (s, w, None),
                        None => (".".to_owned(), 1, Some(CellMark::Control)),
                    }
                }
            };
            let len16 = s.encode_utf16().count();
            if let Some(m) = mark {
                marks.push((u16pos..u16pos + len16, m));
            }
            text += &s;
            u16pos += len16;
            if width < 2 {
                text.push(' ');
                u16pos += 1;
            }
        }
        RowText { text, marks }
    }
}

/// 範囲 `range` と重なる行内の文字の番号の範囲。
pub fn cells_in(row: &CodeRow, range: &Range<u64>) -> Option<Range<usize>> {
    let i0 = row.cells.iter().position(|c| c.end() > range.start)?;
    let i1 = row.cells.iter().rposition(|c| c.start < range.end)? + 1;
    (i0 < i1).then_some(i0..i1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(bytes: &[u8]) -> Snapshot {
        Snapshot::from_bytes(bytes)
    }

    fn starts(rows: &[CodeRow]) -> Vec<u64> {
        rows.iter().map(|r| r.start).collect()
    }

    #[test]
    fn splits_rows_at_newlines_and_every_eight_characters() {
        let s = snap("0123456789\nあい\n".as_bytes());
        let rows = rows_from(&s, 0, 10);
        // "01234567" | "89\n" | "あい\n" | 終わりの空の行
        assert_eq!(starts(&rows), vec![0, 8, 11, 18]);
        assert_eq!(
            rows.iter().map(|r| r.line).collect::<Vec<_>>(),
            vec![Some(1), None, Some(2), Some(3)]
        );
        assert_eq!(rows[2].cells[0].kind, CellKind::Char('あ'));
        assert!(rows[3].cells.is_empty());
        assert_eq!(locate(&rows, 18, s.len()), Some((3, 0)));
        assert_eq!(locate(&rows, 14, s.len()), Some((2, 1)));
        // 行の先頭
        for (pos, want) in [
            (0, 0),
            (7, 0),
            (8, 8),
            (10, 8),
            (11, 11),
            (17, 11),
            (18, 18),
        ] {
            assert_eq!(row_start_at(&s, pos), want, "pos {pos}");
        }
        assert_eq!(next_row(&s, 8), Some(11));
        assert_eq!(next_row(&s, 18), None);
        assert_eq!(prev_row(&s, 11), Some(8));
        assert_eq!(prev_row(&s, 0), None);
    }

    #[test]
    fn end_of_document_without_newline() {
        let s = snap(b"abc");
        let rows = rows_from(&s, 0, 5);
        assert_eq!(starts(&rows), vec![0]);
        assert_eq!(locate(&rows, 3, 3), Some((0, 3)));
        assert_eq!(row_start_at(&s, 3), 0);
        // 8 文字ちょうどなら終わりの位置は次の行
        let s = snap(b"abcdefgh");
        let rows = rows_from(&s, 0, 5);
        assert_eq!(starts(&rows), vec![0, 8]);
        assert_eq!(locate(&rows, 8, 8), Some((1, 0)));
        assert_eq!(row_start_at(&s, 8), 8);
        // 空の文書
        let s = snap(b"");
        assert_eq!(starts(&rows_from(&s, 0, 5)), vec![0]);
        assert_eq!(locate(&rows_from(&s, 0, 5), 0, 0), Some((0, 0)));
    }

    #[test]
    fn invalid_bytes_and_character_steps() {
        let esc = yy_encoding::escape_char(0x82).to_string();
        let mut bytes = b"a".to_vec();
        bytes.extend_from_slice(esc.as_bytes());
        bytes.push(0xFF);
        bytes.extend_from_slice("😀".as_bytes());
        let s = snap(&bytes);
        let rows = rows_from(&s, 0, 2);
        let kinds: Vec<_> = rows[0].cells.iter().map(|c| c.kind).collect();
        assert_eq!(
            kinds,
            vec![
                CellKind::Char('a'),
                CellKind::Byte(0x82),
                CellKind::Byte(0xFF),
                CellKind::Char('😀')
            ]
        );
        assert_eq!(codes_text(&rows[0].cells), "0061 \\x82 \\xFF 1F600");
        assert_eq!(next_char(&s, 1), 5);
        assert_eq!(prev_char(&s, 5), 1);
        assert_eq!(prev_char(&s, 6), 5);
        assert_eq!(prev_char(&s, 10), 6);
        assert_eq!(align(&s, 8), 6);
    }

    #[test]
    fn long_lines_are_split_from_a_window() {
        let text = "あ".repeat(100_000);
        let s = snap(text.as_bytes());
        let pos = 3 * 90_000;
        let top = row_start_at(&s, pos);
        assert!(top <= pos && pos - top < 8 * 3, "{top}");
        assert_eq!(top % 3, 0);
        let rows = rows_from(&s, top, 3);
        assert!(locate(&rows, pos, s.len()).is_some());
        assert_eq!(prev_row(&s, top), Some(top - 24));
        // 区切りの位置を含む文字で行を改める（どこから数えても同じ行になる）
        let b = align(&s, 4 * LINE_WINDOW);
        assert_eq!(row_start_at(&s, b), b);
        assert_eq!(row_start_at(&s, b + 3), b);
        let before = row_start_at(&s, b - 3);
        assert!(b - before <= 24);
        assert_eq!(next_row(&s, before), Some(b));
        assert_eq!(next_row(&s, row_start_at(&s, b - 30)), Some(before));
    }

    #[test]
    fn formats_rows() {
        let l = CodeLayout::for_lines(10);
        let s = snap("あA\t".as_bytes());
        let rows = rows_from(&s, 0, 1);
        let r = l.format_row(&rows[0], None, false);
        let mut want = "     1  3042   0041   0009".to_owned();
        want += &" ".repeat(l.char_col(0) - want.len());
        assert_eq!(r.text, want + "あA . ");
        let dot = r.text.encode_utf16().count() - 2;
        assert_eq!(r.marks, vec![(dot..dot + 1, CellMark::Control)]);
        // 入力中の値は文字の代わりに表示する（文書の終わりでも）
        let r = l.format_row(&rows[0], Some((3, "30")), false);
        assert!(r.text.contains("0009   30 "));
        assert_eq!(l.hit(l.code_col(1) + 3), Some((Pane::Hex, 1)));
        assert_eq!(l.hit(l.code_col(4) - 1), Some((Pane::Hex, 3)));
        assert_eq!(l.hit(l.char_col(2) + 1), Some((Pane::Ascii, 2)));
        assert_eq!(l.hit(0), None);
    }

    #[test]
    fn parses_codes_and_digits() {
        assert_eq!(
            parse_codes("3042 U+3044, 0x41 \\u{1F600}").as_deref(),
            Some("あいA😀")
        );
        assert_eq!(parse_codes("D800"), None);
        assert_eq!(parse_codes("hello"), None);
        assert_eq!(parse_codes(""), None);
        let mut e = None;
        for d in [1, 0, 0xF, 6, 0, 0] {
            e = push_digit(e, d);
        }
        assert_eq!(e, Some((0x10F600, 6)));
        assert_eq!(push_digit(e, 1), None);
        assert_eq!(push_digit(Some((0x11000, 5)), 0), None);
    }

    #[test]
    fn cells_in_range() {
        let s = snap(b"abcdef");
        let rows = rows_from(&s, 0, 1);
        assert_eq!(cells_in(&rows[0], &(1..3)), Some(1..3));
        assert_eq!(cells_in(&rows[0], &(6..9)), None);
    }
}
