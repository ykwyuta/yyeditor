//! 16 進数（バイナリ）編集モード（08 章 4 の拡張候補）。
//!
//! 文書のバイト列を 1 行 16 バイトの 16 進ダンプとして表示・編集する。文書（ピースツリー）は
//! テキスト編集と同じものを使い、行の索引を使わずにバイト位置から直接行を決めるため、
//! 数 GB のファイルでも任意の位置をすぐに表示できる。
//!
//! 表示の形式（`digits` はオフセットの桁数）:
//!
//! ```text
//! 00000000  48 65 6C 6C 6F 20 57 6F  72 6C 64 0A 00 01 02 03  Hello World.....
//! ```

use std::ops::Range;

use crate::edit::Change;

/// 1 行のバイト数。
pub const ROW_BYTES: u64 = 16;

/// カーソルのある欄。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Pane {
    #[default]
    Hex,
    Ascii,
}

/// 16 進ダンプの桁の配置（等幅フォントの桁）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HexLayout {
    /// オフセットの桁数
    pub digits: usize,
}

impl HexLayout {
    /// 長さ `len` の文書に合わせたオフセットの桁数（8 桁以上）。
    pub fn for_len(len: u64) -> HexLayout {
        let bits = 64 - len.max(1).leading_zeros() as usize;
        HexLayout {
            digits: bits.div_ceil(4).max(8),
        }
    }

    /// 行内の `i` 番目のバイトの 16 進表示の先頭の桁。8 バイトごとに空白を 1 つ多く空ける。
    pub fn hex_col(&self, i: usize) -> usize {
        self.digits + 2 + i * 3 + usize::from(i >= 8)
    }

    /// 行内の `i` 番目のバイトの文字表示の桁。
    pub fn ascii_col(&self, i: usize) -> usize {
        self.hex_col(16) + 1 + i
    }

    /// 1 行の桁数。
    pub fn width(&self) -> usize {
        self.ascii_col(16)
    }

    /// 1 行の表示テキスト。`bytes` は行の内容（最後の行は 16 バイト未満）。
    pub fn format_row(&self, offset: u64, bytes: &[u8]) -> String {
        let mut s = format!("{offset:0width$X}  ", width = self.digits);
        for i in 0..ROW_BYTES as usize {
            if i == 8 {
                s.push(' ');
            }
            match bytes.get(i) {
                Some(b) => s += &format!("{b:02X} "),
                None => s += "   ",
            }
        }
        s.push(' ');
        for &b in bytes {
            s.push(printable(b));
        }
        s
    }

    /// 桁 `col` の位置にあるもの（欄, 行内のバイト番号, 16 進の下位の桁か）。
    /// バイトの間の空白は近いほうのバイトにする。オフセット欄なら `None`。
    pub fn hit(&self, col: usize) -> Option<(Pane, usize, u8)> {
        if col < self.hex_col(0) {
            return None;
        }
        if col >= self.ascii_col(0) {
            return Some((Pane::Ascii, (col - self.ascii_col(0)).min(15), 0));
        }
        for i in 0..16 {
            let c = self.hex_col(i);
            if col < c + 3 {
                return Some((Pane::Hex, i, u8::from(col > c)));
            }
            if i == 7 && col < self.hex_col(8) {
                return Some((Pane::Hex, 8, 0));
            }
        }
        Some((Pane::Hex, 15, 1))
    }
}

/// 文字の欄の 1 バイトの表示。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CharCell {
    /// 文字の先頭のバイト（表示する文字列、表示幅（桁）、文字のバイト数）
    Char {
        text: String,
        width: usize,
        len: usize,
    },
    /// 文字の 2 バイト目以降
    Cont,
    /// 制御文字など、表示しない文字（`.`）
    Control,
    /// 文字コードとして不正なバイト（`.`）
    Invalid,
}

/// 文字の欄の文字コード。`None` は ASCII（印字できる ASCII 以外は `.`）。
pub type Charset = Option<yy_encoding::Encoding>;

/// 文字の欄に表示する文字を決めるために、表示する行より前から読むバイト数
/// （多バイト文字の途中から始まらないように）。
pub const CONTEXT_BYTES: u64 = 32;
/// 表示する行より後ろに読むバイト数（行末をまたぐ文字を読み切るため）。
pub const LOOKAHEAD_BYTES: u64 = 8;

/// `data`（ファイル内の位置 `base` から）を文字コード `charset` で読み、各バイトの文字の欄の表示を
/// 返す。先頭部分は多バイト文字・シフト状態の区切りを合わせるための文脈として読み、
/// 結果は `data` 全体の各バイトについて返す（呼び出し側が必要な範囲を使う）。
///
/// `ambiguous_wide` なら東アジアの曖昧幅文字（①・○ など）を 2 桁とする（エディタの桁の数え方と
/// 同じ）。ただし 1 バイトの文字は 1 桁にする（1 バイトの文字コードのギリシャ文字など）。
pub fn char_cells(charset: Charset, data: &[u8], base: u64, ambiguous_wide: bool) -> Vec<CharCell> {
    use yy_encoding::Encoding;
    let Some(enc) = charset else {
        return data
            .iter()
            .map(|&b| match printable(b) {
                '.' if b != b'.' => CharCell::Control,
                c => CharCell::Char {
                    text: c.to_string(),
                    width: 1,
                    len: 1,
                },
            })
            .collect();
    };
    let mut cells = vec![CharCell::Control; data.len()];
    if enc == Encoding::Utf8 {
        let mut pos = 0;
        for chunk in data.utf8_chunks() {
            for (i, c) in chunk.valid().char_indices() {
                set_char(
                    &mut cells,
                    pos + i,
                    c.len_utf8(),
                    &c.to_string(),
                    ambiguous_wide,
                );
            }
            pos += chunk.valid().len();
            for _ in chunk.invalid() {
                cells[pos] = CharCell::Invalid;
                pos += 1;
            }
        }
        return cells;
    }
    // UTF-16・32 は符号単位の境界から読む
    let unit = match enc {
        Encoding::Utf16Le | Encoding::Utf16Be => 2,
        Encoding::Utf32Le | Encoding::Utf32Be => 4,
        _ => 1,
    };
    let start = ((unit - base % unit) % unit) as usize;
    let ebcdic = enc.records().is_some();
    // EBCDIC のレコードの区切り（固定長など）は表示に関係しないので、改行 NL として読む
    let mut dec = enc.with_records(yy_encoding::Records::Nl).new_decoder(true);
    let mut out = Vec::new();
    let mut pending = start;
    for i in start..data.len() {
        out.clear();
        dec.decode(&data[i..i + 1], &mut out, false);
        if out.is_empty() {
            continue;
        }
        let text = String::from_utf8_lossy(&out).into_owned();
        assign(
            &mut cells,
            data,
            pending..i + 1,
            &text,
            ebcdic,
            ambiguous_wide,
        );
        pending = i + 1;
    }
    cells
}

/// 範囲 `span` のバイトをデコードした結果 `text` を各バイトに割り当てる。
fn assign(
    cells: &mut [CharCell],
    data: &[u8],
    span: Range<usize>,
    text: &str,
    ebcdic: bool,
    ambiguous_wide: bool,
) {
    let mut chars: Vec<char> = text.chars().collect();
    let (mut a, mut b) = (span.start, span.end);
    // 不正なバイト（エスケープ文字）は 1 バイトずつ、先頭と末尾から割り当てる
    while a < b && chars.first().is_some_and(|&c| is_invalid(c)) {
        cells[a] = CharCell::Invalid;
        chars.remove(0);
        a += 1;
    }
    while a < b && chars.last().is_some_and(|&c| is_invalid(c)) {
        b -= 1;
        cells[b] = CharCell::Invalid;
        chars.pop();
    }
    // EBCDIC の SO（0x0E）・SI（0x0F）は文字にならない
    while ebcdic && b - a > 1 && matches!(data[a], 0x0E | 0x0F) {
        cells[a] = CharCell::Control;
        a += 1;
    }
    if a >= b {
        return;
    }
    if chars.is_empty() {
        for c in &mut cells[a..b] {
            *c = CharCell::Control;
        }
        return;
    }
    let s: String = chars.into_iter().collect();
    set_char(cells, a, b - a, &s, ambiguous_wide);
}

fn is_invalid(c: char) -> bool {
    yy_encoding::unescape_char(c).is_some() || c == char::REPLACEMENT_CHARACTER
}

/// 位置 `at` から `len` バイトの文字 `s` を割り当てる。
fn set_char(cells: &mut [CharCell], at: usize, len: usize, s: &str, ambiguous_wide: bool) {
    use unicode_width::UnicodeWidthStr;
    let invisible = s.chars().all(|c| {
        c.is_control()
            || matches!(c, '\u{200B}'..='\u{200F}' | '\u{2028}'..='\u{202E}' | '\u{2060}'..='\u{2064}' | '\u{FEFF}')
    });
    if invisible {
        cells[at] = CharCell::Control;
    } else {
        let width = if ambiguous_wide && len >= 2 {
            UnicodeWidthStr::width_cjk(s)
        } else {
            UnicodeWidthStr::width(s)
        };
        // 結合文字だけのときは ◌ に付けて表示する
        let text = if width == 0 {
            format!("\u{25CC}{s}")
        } else {
            s.to_owned()
        };
        cells[at] = CharCell::Char {
            text,
            width: width.max(1),
            len,
        };
    }
    for c in &mut cells[at + 1..at + len] {
        *c = CharCell::Cont;
    }
}

/// 文字の欄に入力・貼り付けする文字列のバイト列。`charset` が `None`（ASCII）なら UTF-8。
/// 変換できない文字があれば `None`。
pub fn encode_text(charset: Charset, text: &str) -> Option<Vec<u8>> {
    match charset {
        None => Some(text.as_bytes().to_vec()),
        Some(enc) => yy_encoding::encode_all(
            enc.with_records(yy_encoding::Records::Nl),
            text.as_bytes(),
            yy_encoding::EscapeMode::Literal,
        )
        .ok(),
    }
}

/// 文字の欄からコピーする文字列（`charset` で読む。不正なバイトは U+FFFD）。
pub fn decode_text(charset: Charset, bytes: &[u8]) -> String {
    match charset {
        None => String::from_utf8_lossy(bytes).into_owned(),
        Some(enc) => {
            let (out, _) =
                yy_encoding::decode_all(enc.with_records(yy_encoding::Records::Nl), bytes, false);
            String::from_utf8_lossy(&out).into_owned()
        }
    }
}

/// 文字の欄で色を変える範囲の種類。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellMark {
    Control,
    Invalid,
}

/// 1 行の表示テキストと、文字の欄で色を変える範囲（UTF-16 の位置）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowText {
    pub text: String,
    pub marks: Vec<(Range<usize>, CellMark)>,
}

impl HexLayout {
    /// 1 行の表示テキスト（文字の欄は `cells` で表示する）。`cells` は `bytes` の各バイトの表示。
    ///
    /// 文字はその先頭のバイトの桁に置き、残りのバイトの桁は空白にするので、各バイトの桁の位置は
    /// 変わらない。行末をまたぐ文字は、はみ出して表示する。
    pub fn format_row_cells(&self, offset: u64, bytes: &[u8], cells: &[CharCell]) -> RowText {
        // オフセットと 16 進の欄（文字の欄の手前まで）
        let mut text = self.format_row(offset, bytes);
        text.truncate(self.ascii_col(0));
        let mut u16pos = text.len(); // ここまでは ASCII
        let mut marks = Vec::new();
        let mut covered = 0; // この位置までは前の文字が覆っている
        let n = bytes.len();
        let push = |text: &mut String, s: &str, u16pos: &mut usize| {
            text.push_str(s);
            *u16pos += s.encode_utf16().count();
        };
        for i in 0..n {
            match cells.get(i).unwrap_or(&CharCell::Control) {
                CharCell::Char {
                    text: s,
                    width,
                    len,
                } => {
                    let avail = (*len).min(n - i);
                    let continues = i + len > n;
                    if *width <= avail || continues {
                        push(&mut text, s, &mut u16pos);
                        push(
                            &mut text,
                            &" ".repeat(avail.saturating_sub(*width)),
                            &mut u16pos,
                        );
                    } else {
                        // 桁が足りない（幅の広い 1 バイト文字など）
                        marks.push((u16pos..u16pos + 1, CellMark::Control));
                        push(&mut text, ".", &mut u16pos);
                        push(&mut text, &" ".repeat(avail - 1), &mut u16pos);
                    }
                    covered = i + avail;
                }
                CharCell::Cont => {
                    if i >= covered {
                        push(&mut text, " ", &mut u16pos);
                    }
                }
                cell @ (CharCell::Control | CharCell::Invalid) => {
                    let mark = if *cell == CharCell::Invalid {
                        CellMark::Invalid
                    } else {
                        CellMark::Control
                    };
                    marks.push((u16pos..u16pos + 1, mark));
                    push(&mut text, ".", &mut u16pos);
                }
            }
        }
        RowText { text, marks }
    }
}

/// 文字表示の欄の文字（印字できない ASCII 以外は `.`）。
pub fn printable(b: u8) -> char {
    if (0x20..0x7F).contains(&b) {
        b as char
    } else {
        '.'
    }
}

/// バイト列の 16 進表記（`48 65 6C`）。
pub fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 {
            s.push(' ');
        }
        s += &format!("{b:02X}");
    }
    s
}

/// 16 進表記を読む（空白・カンマ・`0x` の区切りは無視）。奇数桁・16 進以外の文字なら `None`。
pub fn parse_hex(text: &str) -> Option<Vec<u8>> {
    let mut digits = Vec::new();
    for tok in text.split(|c: char| c.is_whitespace() || c == ',' || c == ';') {
        let tok = tok
            .strip_prefix("0x")
            .or_else(|| tok.strip_prefix("0X"))
            .unwrap_or(tok);
        for c in tok.chars() {
            digits.push(c.to_digit(16)? as u8);
        }
    }
    if digits.is_empty() || digits.len() % 2 != 0 {
        return None;
    }
    Some(digits.chunks(2).map(|p| p[0] << 4 | p[1]).collect())
}

/// オフセットの入力（`0x1F`・`1Fh`・`$1F` は 16 進、それ以外は 10 進。`_`・`,` は無視）。
pub fn parse_offset(text: &str) -> Option<u64> {
    let t: String = text
        .trim()
        .chars()
        .filter(|c| *c != '_' && *c != ',')
        .collect();
    let lower = t.to_ascii_lowercase();
    if let Some(h) = lower
        .strip_prefix("0x")
        .or_else(|| lower.strip_prefix('$'))
        .or_else(|| lower.strip_suffix('h'))
    {
        return u64::from_str_radix(h, 16).ok();
    }
    lower.parse().ok()
}

/// 16 進の欄で数字 `digit`（0〜15）を入力する変更。
///
/// `at` のバイトの `nibble`（0 = 上位, 1 = 下位）の桁を書き換える。上書きでない（挿入）場合、
/// 上位の桁の入力は新しいバイトを挿入する。文書の終わりでは追加する。
/// 戻り値は（変更, 入力後のカーソル位置, 入力後の桁）。
pub fn type_nibble(
    byte: Option<u8>,
    at: u64,
    nibble: u8,
    digit: u8,
    overwrite: bool,
) -> (Change, u64, u8) {
    let d = digit & 0x0F;
    match (byte, nibble, overwrite) {
        // 挿入モードの上位の桁、または文書の終わり: 新しいバイト
        (None, _, _) | (Some(_), 0, false) => (Change::replace_bytes(at..at, vec![d << 4]), at, 1),
        (Some(b), 0, true) => (
            Change::replace_bytes(at..at + 1, vec![(b & 0x0F) | d << 4]),
            at,
            1,
        ),
        (Some(b), _, _) => (
            Change::replace_bytes(at..at + 1, vec![(b & 0xF0) | d]),
            at + 1,
            0,
        ),
    }
}

/// 文字の欄でバイト列 `bytes` を入力する変更（上書きなら同じ長さを置き換える）。
/// 戻り値は（変更, 入力後のカーソル位置）。
pub fn type_bytes(len: u64, at: u64, bytes: &[u8], overwrite: bool) -> (Change, u64) {
    let n = bytes.len() as u64;
    let end = if overwrite { (at + n).min(len) } else { at };
    (Change::replace_bytes(at..end, bytes.to_vec()), at + n)
}

/// 選択範囲（空なら `at` のバイト）を削除する範囲。`backward` なら `at` の直前のバイト。
pub fn delete_range(sel: Range<u64>, len: u64, backward: bool) -> Option<Range<u64>> {
    if !sel.is_empty() {
        return Some(sel);
    }
    let at = sel.start;
    if backward {
        (at > 0).then(|| at - 1..at)
    } else {
        (at < len).then(|| at..at + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit;
    use yy_buffer::Snapshot;

    #[test]
    fn formats_rows() {
        let l = HexLayout::for_len(100);
        assert_eq!(l.digits, 8);
        let row = l.format_row(0x10, b"Hello World\n\x00\x01\x02\x7F");
        assert_eq!(
            row,
            "00000010  48 65 6C 6C 6F 20 57 6F  72 6C 64 0A 00 01 02 7F  Hello World....."
        );
        assert_eq!(row.len(), l.width());
        // 最後の短い行でも文字欄の位置はそろう
        let short = l.format_row(0x20, b"AB");
        assert_eq!(short.find("AB").unwrap(), l.ascii_col(0));
        assert_eq!(&short[l.hex_col(1)..l.hex_col(1) + 2], "42");
        // 4 GB を超える文書はオフセットの桁を増やす
        assert_eq!(HexLayout::for_len((1 << 36) - 1).digits, 9);
        assert_eq!(HexLayout::for_len(1 << 36).digits, 10);
    }

    fn row(charset: Charset, data: &[u8], base: u64, skip: usize, n: usize) -> RowText {
        let cells = char_cells(charset, data, base, true);
        let l = HexLayout { digits: 8 };
        l.format_row_cells(
            base + skip as u64,
            &data[skip..skip + n],
            &cells[skip..skip + n],
        )
    }

    /// 文字の欄の部分。
    fn chars(r: &RowText) -> String {
        let l = HexLayout { digits: 8 };
        r.text[l.ascii_col(0)..].to_owned()
    }

    #[test]
    fn ascii_cells_match_format_row() {
        let l = HexLayout { digits: 8 };
        let data = b"Hello World\n\x00\x01\x02\x7F";
        let r = l.format_row_cells(0x10, data, &char_cells(None, data, 0x10, true));
        assert_eq!(r.text, l.format_row(0x10, data));
        assert_eq!(r.marks.len(), 5);
        assert_eq!(
            r.marks[0],
            (l.ascii_col(11)..l.ascii_col(12), CellMark::Control)
        );
    }

    #[test]
    fn multibyte_characters_keep_byte_columns() {
        use yy_encoding::Encoding;
        // CP932: 「日本」（2 バイトずつ）と半角カナ・ASCII
        let sjis = b"\x93\xFA\x96\x7B\xB1A";
        let r = row(Some(Encoding::Cp932), sjis, 0, 0, sjis.len());
        assert_eq!(chars(&r), "日本ｱA");
        // UTF-8: 3 バイトの文字は幅 2 の後に空白 1 つ
        let utf8 = "日本a".as_bytes();
        let r = row(Some(Encoding::Utf8), utf8, 0, 0, utf8.len());
        assert_eq!(chars(&r), "日 本 a");
        // 不正なバイトは色を変えた `.`
        let bad = b"a\xFFb";
        let r = row(Some(Encoding::Utf8), bad, 0, 0, 3);
        assert_eq!(chars(&r), "a.b");
        assert_eq!(r.marks.last().unwrap().1, CellMark::Invalid);
        let r = row(Some(Encoding::Cp932), b"\x82\x20x", 0, 0, 3);
        assert_eq!(chars(&r), ". x");
        // 曖昧幅の文字（①）は 2 桁、1 バイトの文字コードのギリシャ文字は 1 桁
        let r = row(Some(Encoding::Cp932), b"\x87\x40\x87\x41x", 0, 0, 5);
        assert_eq!(chars(&r), "①②x");
        let greek = Encoding::from_name("ISO-8859-7").unwrap();
        let r = row(Some(greek), b"\xE1\xE2x", 0, 0, 3);
        assert_eq!(chars(&r), "αβx");
        // UTF-16LE: 改行（2 バイト）は制御文字の `.` と空白
        let u16 = b"A\x00\xE5\x65\n\x00";
        let r = row(Some(Encoding::Utf16Le), u16, 0, 0, 6);
        assert_eq!(chars(&r), "A 日. ");
    }

    #[test]
    fn characters_crossing_rows_are_shown_once() {
        use yy_encoding::Encoding;
        // 行（16 バイト）の境界をまたぐ 3 バイトの文字: 前の行の最後に表示し、次の行は空白
        let mut data = vec![b'x'; 15];
        data.extend_from_slice("あい".as_bytes());
        let cells = char_cells(Some(Encoding::Utf8), &data, 0, true);
        let l = HexLayout { digits: 8 };
        let first = l.format_row_cells(0, &data[..16], &cells[..16]);
        let second = l.format_row_cells(16, &data[16..], &cells[16..]);
        assert_eq!(chars(&first), format!("{}あ", "x".repeat(15)));
        assert_eq!(chars(&second), "  い ");
        // 文脈から読むので、行の途中から読み始めても同じ
        let later = char_cells(Some(Encoding::Utf8), &data[10..], 10, true);
        assert_eq!(later[5..], cells[15..]);
    }

    #[test]
    fn utf16_aligns_to_code_units_and_ebcdic_tracks_shift_state() {
        use yy_encoding::{Ccsid, Encoding, Records};
        // 奇数の位置から渡しても符号単位の境界から読む
        let u16 = b"\x00A\x00B\x00C";
        let cells = char_cells(Some(Encoding::Utf16Be), &u16[1..], 1, true);
        assert_eq!(cells[0], CharCell::Control);
        assert!(matches!(&cells[1], CharCell::Char { text, len: 2, .. } if text == "B"));
        // IBM-930: SO「日本」SI は SO・SI を制御文字、2 バイトずつ文字に
        let enc = Encoding::Ebcdic(Ccsid::Ibm930, Records::Fixed(80));
        let bytes =
            yy_encoding::encode_all(enc, "A日本B".as_bytes(), yy_encoding::EscapeMode::Literal)
                .unwrap();
        let r = row(Some(enc), &bytes, 0, 0, bytes.len());
        assert_eq!(chars(&r), "A.日本.B");
    }

    #[test]
    fn encodes_and_decodes_typed_text() {
        use yy_encoding::Encoding;
        assert_eq!(
            encode_text(Some(Encoding::Cp932), "日"),
            Some(b"\x93\xFA".to_vec())
        );
        assert_eq!(encode_text(Some(Encoding::Cp932), "😀"), None);
        assert_eq!(encode_text(None, "é"), Some("é".as_bytes().to_vec()));
        assert_eq!(
            decode_text(Some(Encoding::Cp932), b"\x93\xFA\x96\x7B"),
            "日本"
        );
        assert_eq!(decode_text(Some(Encoding::Utf16Le), b"A\x00"), "A");
    }

    #[test]
    fn hit_test_columns() {
        let l = HexLayout { digits: 8 };
        assert_eq!(l.hit(0), None);
        assert_eq!(l.hit(l.hex_col(0)), Some((Pane::Hex, 0, 0)));
        assert_eq!(l.hit(l.hex_col(0) + 1), Some((Pane::Hex, 0, 1)));
        assert_eq!(l.hit(l.hex_col(3) + 2), Some((Pane::Hex, 3, 1)));
        assert_eq!(l.hit(l.hex_col(8) - 1), Some((Pane::Hex, 8, 0)));
        assert_eq!(l.hit(l.hex_col(15) + 1), Some((Pane::Hex, 15, 1)));
        assert_eq!(l.hit(l.ascii_col(0)), Some((Pane::Ascii, 0, 0)));
        assert_eq!(l.hit(l.ascii_col(15) + 5), Some((Pane::Ascii, 15, 0)));
    }

    #[test]
    fn parses_hex_and_offsets() {
        assert_eq!(parse_hex("48 65 6c"), Some(vec![0x48, 0x65, 0x6C]));
        assert_eq!(parse_hex("0x48,0x65"), Some(vec![0x48, 0x65]));
        assert_eq!(parse_hex("4865"), Some(vec![0x48, 0x65]));
        assert_eq!(parse_hex("486"), None);
        assert_eq!(parse_hex("zz"), None);
        assert_eq!(parse_hex(""), None);
        assert_eq!(to_hex(&[0x48, 0x0A]), "48 0A");
        assert_eq!(parse_offset("0x1F"), Some(31));
        assert_eq!(parse_offset("1Fh"), Some(31));
        assert_eq!(parse_offset("$ff"), Some(255));
        assert_eq!(parse_offset("1,024"), Some(1024));
        assert_eq!(parse_offset("x"), None);
    }

    fn apply(text: &[u8], c: Change) -> Vec<u8> {
        let s = Snapshot::from_bytes(text.to_vec());
        let a = edit::apply(&s, vec![c]);
        a.snapshot.read(0..a.snapshot.len())
    }

    #[test]
    fn typing_nibbles() {
        let data = b"\x12\x34";
        // 上書き: 上位 → 下位の桁、次のバイトへ
        let (c, at, n) = type_nibble(Some(0x12), 0, 0, 0xA, true);
        assert_eq!((apply(data, c), at, n), (b"\xA2\x34".to_vec(), 0, 1));
        let (c, at, n) = type_nibble(Some(0xA2), 0, 1, 0xB, true);
        assert_eq!((apply(b"\xA2\x34", c), at, n), (b"\xAB\x34".to_vec(), 1, 0));
        // 挿入: 上位の桁で新しいバイトを挿入し、下位の桁はそのバイトを書き換える
        let (c, at, n) = type_nibble(Some(0x12), 0, 0, 0xF, false);
        assert_eq!((apply(data, c), at, n), (b"\xF0\x12\x34".to_vec(), 0, 1));
        let (c, at, n) = type_nibble(Some(0xF0), 0, 1, 0x1, false);
        assert_eq!(
            (apply(b"\xF0\x12\x34", c), at, n),
            (b"\xF1\x12\x34".to_vec(), 1, 0)
        );
        // 文書の終わりでは追加する
        let (c, at, n) = type_nibble(None, 2, 0, 0x5, true);
        assert_eq!((apply(data, c), at, n), (b"\x12\x34\x50".to_vec(), 2, 1));
    }

    #[test]
    fn typing_bytes_and_deleting() {
        let (c, at) = type_bytes(3, 1, b"XY", true);
        assert_eq!((apply(b"abc", c), at), (b"aXY".to_vec(), 3));
        let (c, at) = type_bytes(3, 2, b"XY", true);
        assert_eq!((apply(b"abc", c), at), (b"abXY".to_vec(), 4));
        let (c, _) = type_bytes(3, 1, b"XY", false);
        assert_eq!(apply(b"abc", c), b"aXYbc");
        assert_eq!(delete_range(2..2, 3, true), Some(1..2));
        assert_eq!(delete_range(2..2, 3, false), Some(2..3));
        assert_eq!(delete_range(3..3, 3, false), None);
        assert_eq!(delete_range(0..0, 3, true), None);
        assert_eq!(delete_range(1..3, 3, true), Some(1..3));
    }
}
