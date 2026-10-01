//! 固定長レコードの表示（16 進数表示の一形態）。
//!
//! ファイルのバイト列を `len` バイトずつのレコードに分け、1 レコードを 3 行で表示する。
//! 上に先頭からのバイト位置（1 始まり）の目盛りを置き、各バイトの桁をそろえて、
//! レコードごとに「コード値（文字コードでの文字の符号）」「16 進数」「文字」の行を並べる。
//!
//! ```text
//!                 1  2  3  4  5  6  7  8
//!      1  C1 C2 SO 4541  SI 12 34
//!         C1 C2 0E 45 41 0F 12 34
//!         A  B     一          .  .
//! ```
//!
//! 文字はレコードごとに読む（EBCDIC のシフト状態はレコードの先頭で 1 バイト部に戻る）。

use crate::hex::{CellMark, CharCell, RowText};

/// 1 バイトの桁数（16 進 2 桁と空白）。
pub const BYTE_COLS: usize = 3;
/// 1 レコードの表示行数（コード値・16 進数・文字）。
pub const LINES: usize = 3;

/// レコードの中の行の種類。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Line {
    Code,
    Hex,
    Char,
}

impl Line {
    pub fn from_index(i: usize) -> Line {
        match i {
            0 => Line::Code,
            1 => Line::Hex,
            _ => Line::Char,
        }
    }
}

/// 固定長表示の桁の配置（等幅フォントの桁）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecordLayout {
    /// レコード番号の桁数
    pub digits: usize,
    /// レコード長（バイト）
    pub len: usize,
}

impl RecordLayout {
    /// `records` 個のレコード（長さ `len`）に合わせた配置（レコード番号は 6 桁以上）。
    pub fn new(records: u64, len: u64) -> RecordLayout {
        RecordLayout {
            digits: records.max(1).to_string().len().max(6),
            len: len.max(1) as usize,
        }
    }

    /// レコード内の `i` 番目のバイトの桁。
    pub fn byte_col(&self, i: usize) -> usize {
        self.digits + 2 + i * BYTE_COLS
    }

    /// 1 行の桁数。
    pub fn width(&self) -> usize {
        self.byte_col(self.len)
    }

    /// 桁 `col` にあるバイトのレコード内の番号（レコード番号の欄なら `None`）。
    pub fn hit(&self, col: usize) -> Option<usize> {
        let c = col.checked_sub(self.byte_col(0))?;
        Some((c / BYTE_COLS).min(self.len - 1))
    }

    /// 目盛りの行（先頭からのバイト位置、1 始まり）。99 バイトまでは全バイト、それより先は
    /// 5 バイトごとに位置を書き、間は `.` にする。
    pub fn header(&self) -> String {
        let mut line: Vec<char> = vec![' '; self.width() + 8];
        for i in 0..self.len {
            let pos = i + 1;
            let col = self.byte_col(i);
            if line[col] != ' ' || line[col + 1] != ' ' {
                continue; // 前の長い番号が掛かっている
            }
            if pos <= 99 {
                let s = format!("{pos:>2}");
                for (k, ch) in s.chars().enumerate() {
                    line[col + k] = ch;
                }
            } else if pos % 5 == 0 {
                for (k, ch) in pos.to_string().chars().enumerate() {
                    line[col + k] = ch;
                }
            } else {
                line[col + 1] = '.';
            }
        }
        let s: String = line.into_iter().collect();
        s.trim_end().to_owned()
    }

    /// レコード `number`（1 始まり）の 3 行。`bytes` はレコードの内容（最後のレコードは短い
    /// ことがある）、`cells` は各バイトの文字の表示、`ebcdic` なら `0E` `0F` の制御を
    /// コード値の行で `SO` `SI` と書く。
    pub fn format_record(
        &self,
        number: u64,
        bytes: &[u8],
        cells: &[CharCell],
        ebcdic: bool,
    ) -> [RowText; LINES] {
        let blank = " ".repeat(self.digits + 2);
        let mut code = Builder::new(format!("{number:>w$}  ", w = self.digits));
        let mut hex = Builder::new(blank.clone());
        let mut chars = Builder::new(blank);
        for (i, &b) in bytes.iter().enumerate() {
            let col = self.byte_col(i);
            hex.put(col, &format!("{b:02X}"), 2, None);
            let cell = cells.get(i).unwrap_or(&CharCell::Control);
            match cell {
                CharCell::Char { text, width, len } => {
                    let end = (i + len).min(bytes.len());
                    let s: String = bytes[i..end].iter().map(|b| format!("{b:02X}")).collect();
                    code.put(col, &s, s.len(), None);
                    chars.put(col, text, *width, None);
                }
                CharCell::Cont => {}
                // シフト（SO / SI）は文字ではないので、文字の行には何も書かない
                CharCell::Control if ebcdic && matches!(b, 0x0E | 0x0F) => {
                    let label = if b == 0x0E { "SO" } else { "SI" };
                    code.put(col, label, 2, Some(CellMark::Control));
                }
                CharCell::Control => {
                    code.put(col, &format!("{b:02X}"), 2, Some(CellMark::Control));
                    chars.put(col, ".", 1, Some(CellMark::Control));
                }
                CharCell::Invalid => {
                    code.put(col, &format!("{b:02X}"), 2, Some(CellMark::Invalid));
                    chars.put(col, ".", 1, Some(CellMark::Invalid));
                }
            }
        }
        [code.finish(), hex.finish(), chars.finish()]
    }
}

/// 桁をそろえて 1 行を組み立てる。
struct Builder {
    text: String,
    /// ここまでの桁数
    col: usize,
    /// ここまでの UTF-16 の長さ
    u16len: usize,
    marks: Vec<(std::ops::Range<usize>, CellMark)>,
}

impl Builder {
    fn new(head: String) -> Builder {
        let n = head.chars().count();
        Builder {
            u16len: head.encode_utf16().count(),
            text: head,
            col: n,
            marks: Vec::new(),
        }
    }

    /// 桁 `col` から幅 `width` 桁の `s` を置く（前の文字が掛かっていれば、その後ろに続ける）。
    fn put(&mut self, col: usize, s: &str, width: usize, mark: Option<CellMark>) {
        while self.col < col {
            self.text.push(' ');
            self.col += 1;
            self.u16len += 1;
        }
        let len16 = s.encode_utf16().count();
        if let Some(m) = mark {
            self.marks.push((self.u16len..self.u16len + len16, m));
        }
        self.text.push_str(s);
        self.col += width;
        self.u16len += len16;
    }

    fn finish(self) -> RowText {
        RowText {
            text: self.text,
            marks: self.marks,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hex::char_cells;
    use yy_encoding::{Ccsid, Encoding, Records};

    #[test]
    fn header_shows_byte_positions() {
        let l = RecordLayout::new(5, 12);
        assert_eq!(l.byte_col(0), 8);
        let h = l.header();
        assert_eq!(&h[..8], "        ");
        assert!(h.starts_with("         1  2  3"));
        assert!(h.ends_with("11 12"));
        // 100 バイトを超えると 5 バイトごと
        let l = RecordLayout::new(5, 106);
        let h = l.header();
        assert_eq!(&h[l.byte_col(98)..l.byte_col(100)], "99 100");
        assert_eq!(&h[l.byte_col(100)..l.byte_col(101)], " . ");
        assert_eq!(&h[l.byte_col(104)..l.byte_col(104) + 3], "105");
        assert_eq!(l.hit(l.byte_col(3) + 2), Some(3));
        assert_eq!(l.hit(0), None);
        assert_eq!(l.hit(10_000), Some(105));
    }

    #[test]
    fn formats_ebcdic_records() {
        let enc = Encoding::Ebcdic(Ccsid::Ibm930, Records::Fixed(8));
        // A B SO 一 SI パック 10 進数 12 34
        let bytes = [0xC1, 0xC2, 0x0E, 0x45, 0x41, 0x0F, 0x12, 0x34];
        let cells = char_cells(Some(enc), &bytes, 0, true);
        let l = RecordLayout::new(1, 8);
        let [code, hex, chars] = l.format_record(1, &bytes, &cells, true);
        assert_eq!(code.text, "     1  C1 C2 SO 4541  SI 12 34");
        assert_eq!(hex.text, "        C1 C2 0E 45 41 0F 12 34");
        assert_eq!(chars.text, "        A  B     一       .  .");
        // 各バイトの桁がそろう
        for i in 0..8 {
            let col = l.byte_col(i);
            assert_ne!(&hex.text[col..col + 1], " ");
        }
        // SO / SI と制御文字（12 = U+0012、34 = U+0094）は色を変える
        assert_eq!(code.marks.len(), 4);
        assert_eq!(chars.marks.len(), 2);
    }
}
