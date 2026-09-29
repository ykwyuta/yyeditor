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
