//! 固定長レコードの表示（16 進数表示の一形態）。
//!
//! ファイルのバイト列を `len` バイトずつのレコードに分け、1 レコードを 3 行で表示する。
//! 上に先頭からのバイト位置（1 始まり）の目盛りを置き、各バイトの桁をそろえて、
//! レコードごとに「コード値（文字の Unicode のコード値）」「16 進数」「文字」の行を並べる。
//!
//! ```text
//!                 1  2  3  4  5  6  7  8
//!      1  41 42 SO 4E00  SI 12 94
//!         C1 C2 0E 45 41 0F 12 34
//!         A  B     一       .  .
//! ```
//!
//! コード値は文字の先頭のバイトの桁から、その文字のバイトの桁に収まる範囲に書く（16 進数で
//! 2 桁以上。結合文字などで複数の文字になるときは `+` でつなぐ）。収まらないとき（1 バイトの
//! 半角カナ `FF71` など）は `**` とし、値はステータスバーに表示する。
//!
//! 文字はレコードごとに読む（EBCDIC のシフト状態はレコードの先頭で 1 バイト部に戻る）。

use crate::hex::{CellMark, CharCell, NO_CHAR, RowText};

/// 文字列 `s` の Unicode のコード値をコード値の行の形（`41`、`4E00`、`304B+309A`）で。
fn compact_code(s: &str) -> String {
    let codes: Vec<String> = s.chars().map(|c| format!("{:02X}", c as u32)).collect();
    codes.join("+")
}

/// 文字列 `s` の Unicode のコード値をステータスバーの形（`U+0041`、`U+304B U+309A`）で。
pub fn unicode_label(s: &str) -> String {
    let codes: Vec<String> = s.chars().map(|c| format!("U+{:04X}", c as u32)).collect();
    codes.join(" ")
}

/// レコードの `i` バイト目を含む文字（`cells` の `Cont` をさかのぼった先頭）の読んだ文字。
/// 文字がない（SO・SI、不正なバイトなど）なら `None`。
pub fn char_at(cells: &[CharCell], i: usize) -> Option<&str> {
    let start = (0..=i.min(cells.len().checked_sub(1)?))
        .rev()
        .find(|&k| cells[k] != CharCell::Cont)?;
    match &cells[start] {
        CharCell::Char { chars, .. } | CharCell::Control(chars) if !chars.is_empty() => Some(chars),
        _ => None,
    }
}

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
    /// コード値の行で `SO` `SI` と書く。コード値の行は Unicode のコード値。
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
            let cell = cells.get(i).unwrap_or(&NO_CHAR);
            // 文字のバイトの桁（間の空白を含み、後ろの区切りの空白は除く）に収まるように書く
            let room = |len: usize| len.min(bytes.len() - i) * BYTE_COLS - 1;
            match cell {
                CharCell::Char {
                    text,
                    width,
                    len,
                    chars: decoded,
                } => {
                    code.put_code(col, room(*len), &compact_code(decoded), None);
                    chars.put(col, text, *width, None);
                }
                CharCell::Cont => {}
                // シフト（SO / SI）は文字ではないので、文字の行には何も書かない
                CharCell::Control(decoded)
                    if decoded.is_empty() && ebcdic && matches!(b, 0x0E | 0x0F) =>
                {
                    let label = if b == 0x0E { "SO" } else { "SI" };
                    code.put(col, label, 2, Some(CellMark::Control));
                }
                CharCell::Control(decoded) => {
                    if !decoded.is_empty() {
                        let len = 1 + cells[i + 1..]
                            .iter()
                            .take_while(|c| **c == CharCell::Cont)
                            .count();
                        code.put_code(
                            col,
                            room(len),
                            &compact_code(decoded),
                            Some(CellMark::Control),
                        );
                    }
                    chars.put(col, ".", 1, Some(CellMark::Control));
                }
                // 不正なバイトは文字ではないので、コード値の行には何も書かない
                CharCell::Invalid => {
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

    /// コード値 `s` を桁 `col` から `room` 桁に収まれば書き、収まらなければ `**` を書く。
    fn put_code(&mut self, col: usize, room: usize, s: &str, mark: Option<CellMark>) {
        if s.len() <= room {
            self.put(col, s, s.len(), mark);
        } else {
            self.put(col, "**", 2, Some(CellMark::Control));
        }
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
        // コード値は Unicode（12 = U+0012、34 = U+0094 の制御文字）
        assert_eq!(code.text, "     1  41 42 SO 4E00  SI 12 94");
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

    #[test]
    fn code_line_fits_unicode_values_to_byte_columns() {
        use yy_encoding::{Ccsid, Encoding, Records};
        // IBM-930 の 1 バイトの半角カナ（U+FF71）は 2 桁に収まらないので `**`
        let enc = Encoding::Ebcdic(Ccsid::Ibm930, Records::Fixed(3));
        let bytes = yy_encoding::encode_all(enc, "ｱ1".as_bytes(), yy_encoding::EscapeMode::Literal)
            .unwrap();
        let cells = char_cells(Some(enc), &bytes, 0, true);
        let l = RecordLayout::new(1, bytes.len() as u64);
        let [code, _, chars] = l.format_record(1, &bytes, &cells, true);
        assert_eq!(code.text, "     1  ** 31");
        assert_eq!(chars.text, "        ｱ  1");
        assert_eq!(
            char_at(&cells, 0).map(unicode_label).as_deref(),
            Some("U+FF71")
        );
        // UTF-8: 3 バイトの文字は 3 バイト分の桁に、不正なバイトは何も書かない
        let bytes = "あa\u{1F600}"
            .as_bytes()
            .iter()
            .copied()
            .chain([0xFF])
            .collect::<Vec<u8>>();
        let cells = char_cells(Some(Encoding::Utf8), &bytes, 0, true);
        let l = RecordLayout::new(1, bytes.len() as u64);
        let [code, hex, _] = l.format_record(1, &bytes, &cells, false);
        assert_eq!(code.text, "     1  3042     61 1F600");
        assert_eq!(&hex.text[l.byte_col(4)..l.byte_col(5)], "F0 ");
        // 4 バイト目（😀 の途中）からも文字をたどれる
        assert_eq!(
            char_at(&cells, 6).map(unicode_label).as_deref(),
            Some("U+1F600")
        );
        assert_eq!(char_at(&cells, 8), None);
        // 結合文字の並び（か゚ = U+304B U+309A）は `+` でつなぐ。収まらなければ `**`
        let enc = Encoding::from_name("Shift_JIS-2004").unwrap();
        let bytes = yy_encoding::encode_all(
            enc,
            "か\u{309A}".as_bytes(),
            yy_encoding::EscapeMode::Literal,
        )
        .unwrap();
        let cells = char_cells(Some(enc), &bytes, 0, true);
        assert_eq!(
            char_at(&cells, 1).map(unicode_label).as_deref(),
            Some("U+304B U+309A")
        );
        let l = RecordLayout::new(1, bytes.len() as u64);
        let [code, _, _] = l.format_record(1, &bytes, &cells, false);
        assert_eq!(code.text, "     1  **");
    }

    #[test]
    fn code_line_shows_ascii_controls() {
        // 文字の欄が ASCII のとき、制御文字は値を色を変えて書き、ASCII 以外は何も書かない
        let bytes = b"A\t\x80";
        let cells = char_cells(None, bytes, 0, true);
        let l = RecordLayout::new(1, 3);
        let [code, _, chars] = l.format_record(1, bytes, &cells, false);
        assert_eq!(code.text, "     1  41 09");
        assert_eq!(code.marks.len(), 1);
        assert_eq!(chars.text, "        A  .  .");
    }
}
