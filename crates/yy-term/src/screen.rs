//! 画面の中身（セル・行・色・文字の属性）。

use unicode_width::UnicodeWidthChar;

/// 色。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Color {
    /// 既定の色（文字色・背景色それぞれの既定）
    #[default]
    Default,
    /// 256 色の番号（0〜15 は基本の 16 色）
    Indexed(u8),
    Rgb(u8, u8, u8),
}

/// 文字の飾り（[`Attr::flags`] のビット）。
pub mod flags {
    pub const BOLD: u16 = 1;
    pub const DIM: u16 = 1 << 1;
    pub const ITALIC: u16 = 1 << 2;
    pub const UNDERLINE: u16 = 1 << 3;
    pub const BLINK: u16 = 1 << 4;
    pub const INVERSE: u16 = 1 << 5;
    pub const HIDDEN: u16 = 1 << 6;
    pub const STRIKE: u16 = 1 << 7;
    pub const DOUBLE_UNDERLINE: u16 = 1 << 8;
}

/// 文字の属性。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Attr {
    pub fg: Color,
    pub bg: Color,
    pub flags: u16,
}

impl Attr {
    pub fn has(&self, f: u16) -> bool {
        self.flags & f != 0
    }
}

/// 1 つのセル。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cell {
    /// 文字（全角の文字の右半分は `' '`）
    pub ch: char,
    /// 結合文字（濁点・異体字セレクタなど。なければ `None`）
    pub combining: Option<Box<str>>,
    /// 幅（1、全角は 2、全角の文字の右半分は 0）
    pub width: u8,
    pub attr: Attr,
}

impl Default for Cell {
    fn default() -> Self {
        Cell::blank(Attr::default())
    }
}

impl Cell {
    /// 空白（消去に使う。背景色は `attr` のもの）。
    pub fn blank(attr: Attr) -> Cell {
        Cell {
            ch: ' ',
            combining: None,
            width: 1,
            attr: Attr {
                fg: attr.fg,
                bg: attr.bg,
                flags: 0,
            },
        }
    }

    /// 全角の文字の右半分か。
    pub fn is_continuation(&self) -> bool {
        self.width == 0
    }

    /// 表示する文字列（結合文字を含む）。
    pub fn text(&self) -> String {
        let mut s = String::new();
        s.push(self.ch);
        if let Some(c) = &self.combining {
            s.push_str(c);
        }
        s
    }
}

/// 1 行。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub cells: Vec<Cell>,
    /// 行の終わりで折り返して次の行に続いている（コピーのとき改行を入れない）
    pub wrapped: bool,
}

impl Line {
    pub fn new(cols: usize, attr: Attr) -> Line {
        Line {
            cells: vec![Cell::blank(attr); cols],
            wrapped: false,
        }
    }

    /// 幅を変える（全角の文字が切れる場合は空白にする）。
    pub fn resize(&mut self, cols: usize) {
        if cols < self.cells.len() {
            self.cells.truncate(cols);
            if let Some(last) = self.cells.last_mut()
                && last.width == 2
            {
                *last = Cell::blank(last.attr);
            }
            self.wrapped = false;
        } else {
            self.cells.resize(cols, Cell::default());
        }
    }

    /// 文字列（行末の空白を除く）。
    pub fn text(&self) -> String {
        let mut s = String::new();
        for c in &self.cells {
            if !c.is_continuation() {
                s.push_str(&c.text());
            }
        }
        s.trim_end_matches(' ').to_owned()
    }

    /// `col` が全角の文字の一部なら、その文字を空白にする（上書きの前に呼ぶ）。
    pub(crate) fn split_wide_at(&mut self, col: usize) {
        let n = self.cells.len();
        if col >= n {
            return;
        }
        if self.cells[col].is_continuation() && col > 0 {
            let a = self.cells[col - 1].attr;
            self.cells[col - 1] = Cell::blank(a);
            self.cells[col] = Cell::blank(a);
        } else if self.cells[col].width == 2 && col + 1 < n {
            let a = self.cells[col].attr;
            self.cells[col + 1] = Cell::blank(a);
        }
    }
}

/// 文字の幅（0〜2）。`ambiguous_wide` なら東アジアの幅が曖昧な文字（○、※ など）を全角とする。
pub fn char_width(c: char, ambiguous_wide: bool) -> usize {
    let w = if ambiguous_wide {
        c.width_cjk()
    } else {
        c.width()
    };
    w.unwrap_or(0)
}

/// 既定の 16 色（Windows Terminal の Campbell に近い配色）。
pub const BASE16: [(u8, u8, u8); 16] = [
    (12, 12, 12),
    (197, 15, 31),
    (19, 161, 14),
    (193, 156, 0),
    (0, 55, 218),
    (136, 23, 152),
    (58, 150, 221),
    (204, 204, 204),
    (118, 118, 118),
    (231, 72, 86),
    (22, 198, 12),
    (249, 241, 165),
    (59, 120, 255),
    (180, 0, 158),
    (97, 214, 214),
    (242, 242, 242),
];

/// 256 色の番号の RGB（0〜15 は `base16`）。
pub fn indexed_rgb(i: u8, base16: &[(u8, u8, u8); 16]) -> (u8, u8, u8) {
    match i {
        0..=15 => base16[i as usize],
        16..=231 => {
            let n = i - 16;
            let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            (level(n / 36), level((n / 6) % 6), level(n % 6))
        }
        _ => {
            let v = 8 + (i - 232) * 10;
            (v, v, v)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette() {
        assert_eq!(indexed_rgb(1, &BASE16), BASE16[1]);
        assert_eq!(indexed_rgb(16, &BASE16), (0, 0, 0));
        assert_eq!(indexed_rgb(231, &BASE16), (255, 255, 255));
        assert_eq!(indexed_rgb(196, &BASE16), (255, 0, 0));
        assert_eq!(indexed_rgb(232, &BASE16), (8, 8, 8));
        assert_eq!(indexed_rgb(255, &BASE16), (238, 238, 238));
    }

    #[test]
    fn widths() {
        assert_eq!(char_width('a', false), 1);
        assert_eq!(char_width('あ', false), 2);
        assert_eq!(char_width('\u{3099}', false), 0);
        assert_eq!(char_width('○', false), 1);
        assert_eq!(char_width('○', true), 2);
    }

    #[test]
    fn resizing_splits_wide_chars() {
        let mut l = Line::new(4, Attr::default());
        l.cells[2] = Cell {
            ch: 'あ',
            combining: None,
            width: 2,
            attr: Attr::default(),
        };
        l.cells[3] = Cell {
            ch: ' ',
            combining: None,
            width: 0,
            attr: Attr::default(),
        };
        l.resize(3);
        assert_eq!(l.cells[2].ch, ' ');
        assert_eq!(l.cells[2].width, 1);
        l.resize(5);
        assert_eq!(l.cells.len(), 5);
    }
}
