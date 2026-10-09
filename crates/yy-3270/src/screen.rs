//! 3270 の画面バッファ（セル・フィールド・属性・カーソル）。
//!
//! バッファは行×桁のセルの並びで、フィールド属性のセル（`fa`）が次のフィールド属性までの
//! 範囲を決める（最後のフィールドは先頭に折り返す）。フィールド属性が 1 つもなければ
//! 書式のない画面（全体が 1 つの非保護の領域）。
//!
//! 文字はホストの符号（EBCDIC）のまま持ち、表示のときに CCSID で変換する（[`Screen::display`]）。
//! 2 バイト文字（DBCS）は、文字セットが DBCS のフィールド、または SO〜SI の間で 2 セルを 1 文字とする。

use yy_encoding::Ccsid;

use crate::codes::*;

/// 拡張属性（色・強調・文字セット）。0 は既定。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ext {
    pub fg: u8,
    pub bg: u8,
    pub hl: u8,
    pub cs: u8,
}

impl Ext {
    /// 種類 `t` の値 `v` を設定する（`XA_ALL` はすべて既定に戻す）。
    pub fn set(&mut self, t: u8, v: u8) {
        match t {
            XA_ALL => *self = Ext::default(),
            XA_FOREGROUND => self.fg = v,
            XA_BACKGROUND => self.bg = v,
            XA_HIGHLIGHTING => self.hl = v,
            XA_CHARSET => self.cs = v,
            _ => {}
        }
    }
}

/// 1 つのセル。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cell {
    /// 文字（ホストの符号。0 は null）
    pub b: u8,
    /// フィールド属性のセルなら、その属性
    pub fa: Option<u8>,
    /// 文字の属性（SA）、またはフィールド属性のセルではフィールドの拡張属性（SFE）
    pub ext: Ext,
}

/// 表示用の 1 セル。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DisplayCell {
    /// 表示する文字（空白・null・属性のセルは `' '`。2 バイト文字の 2 セル目は空文字）
    pub text: String,
    /// 1、または 2（2 バイト文字の 1 セル目）、0（2 セル目）
    pub width: u8,
    /// 色（`0xF1` 青〜`0xF7` 白。拡張属性がなければ 3279 の基本色）
    pub fg: u8,
    pub bg: u8,
    pub hl: u8,
    pub intensified: bool,
    /// 非表示のフィールド（パスワードなど。描かない・コピーしない）
    pub hidden: bool,
    pub protected: bool,
    /// フィールド属性のセル
    pub attribute: bool,
}

/// 3279 の基本色（拡張属性のない画面）。
pub fn base_color(fa: Option<u8>) -> u8 {
    const BLUE: u8 = 0xF1;
    const RED: u8 = 0xF2;
    const GREEN: u8 = 0xF4;
    const WHITE: u8 = 0xF7;
    match fa {
        None => GREEN,
        Some(a) => {
            let intense = a & FA_DISPLAY_MASK == FA_INTENSIFIED;
            match (a & FA_PROTECT != 0, intense) {
                (false, false) => GREEN,
                (false, true) => RED,
                (true, false) => BLUE,
                (true, true) => WHITE,
            }
        }
    }
}

/// 画面バッファ。
#[derive(Clone, Debug)]
pub struct Screen {
    pub rows: usize,
    pub cols: usize,
    /// 既定と代替の大きさ（行, 桁）
    pub default_size: (usize, usize),
    pub alternate_size: (usize, usize),
    pub cells: Vec<Cell>,
    pub cursor: usize,
}

impl Screen {
    pub fn new(default_size: (usize, usize), alternate_size: (usize, usize)) -> Screen {
        let (rows, cols) = default_size;
        Screen {
            rows,
            cols,
            default_size,
            alternate_size,
            cells: vec![Cell::default(); rows * cols],
            cursor: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.cells.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    /// 既定（`alternate` なら代替）の大きさにして、すべて消す。
    pub fn erase(&mut self, alternate: bool) {
        let (rows, cols) = if alternate {
            self.alternate_size
        } else {
            self.default_size
        };
        self.rows = rows;
        self.cols = cols;
        self.cells = vec![Cell::default(); rows * cols];
        self.cursor = 0;
    }

    /// 次のアドレス（折り返す）。
    pub fn next(&self, a: usize) -> usize {
        (a + 1) % self.len()
    }

    pub fn prev(&self, a: usize) -> usize {
        (a + self.len() - 1) % self.len()
    }

    /// 書式のある画面（フィールド属性がある）か。
    pub fn formatted(&self) -> bool {
        self.cells.iter().any(|c| c.fa.is_some())
    }

    /// `a` を含むフィールドのフィールド属性のアドレス（`a` 自身が属性ならそれ）。書式がなければ `None`。
    pub fn field_attr_addr(&self, a: usize) -> Option<usize> {
        let mut p = a;
        for _ in 0..self.len() {
            if self.cells[p].fa.is_some() {
                return Some(p);
            }
            p = self.prev(p);
        }
        None
    }

    /// `a` を含むフィールドの属性。
    pub fn field_attr(&self, a: usize) -> Option<u8> {
        self.field_attr_addr(a).and_then(|p| self.cells[p].fa)
    }

    /// `a` に入力できないか（属性のセル、または保護フィールドの中）。
    pub fn is_protected(&self, a: usize) -> bool {
        if self.cells[a].fa.is_some() {
            return true;
        }
        self.field_attr(a).is_some_and(|fa| fa & FA_PROTECT != 0)
    }

    /// フィールド属性 `fa_addr` のフィールドの中身の範囲（先頭のアドレスと長さ）。
    pub fn field_range(&self, fa_addr: usize) -> (usize, usize) {
        let start = self.next(fa_addr);
        let mut len = 0;
        let mut p = start;
        while self.cells[p].fa.is_none() && len < self.len() {
            len += 1;
            p = self.next(p);
        }
        (start, len)
    }

    /// フィールド属性のアドレスの一覧（アドレス順）。
    pub fn field_attrs(&self) -> Vec<usize> {
        (0..self.len())
            .filter(|&p| self.cells[p].fa.is_some())
            .collect()
    }

    /// `from` より後（`from` は含まない）で最初の、非保護フィールドの先頭の位置（折り返して探す）。
    /// 書式がなければ `None`。
    pub fn next_unprotected(&self, from: usize) -> Option<usize> {
        if !self.formatted() {
            return None;
        }
        let mut p = from;
        for _ in 0..self.len() {
            let q = self.next(p);
            if let Some(fa) = self.cells[p].fa
                && fa & FA_PROTECT == 0
                && self.cells[q].fa.is_none()
            {
                return Some(q);
            }
            p = q;
        }
        None
    }

    /// 最初の非保護フィールドの先頭（なければ 0）。
    pub fn first_unprotected(&self) -> usize {
        self.next_unprotected(self.len() - 1).unwrap_or(0)
    }

    /// `a` を含むフィールドに MDT を立てる。
    pub fn set_mdt(&mut self, a: usize) {
        if let Some(p) = self.field_attr_addr(a)
            && let Some(fa) = self.cells[p].fa.as_mut()
        {
            *fa |= FA_MDT;
        }
    }

    /// すべてのフィールドの MDT を消す。
    pub fn reset_mdt(&mut self) {
        for c in &mut self.cells {
            if let Some(fa) = c.fa.as_mut() {
                *fa &= !FA_MDT;
            }
        }
    }

    /// `a` を含むフィールドが 2 バイト文字（DBCS）のフィールドか。
    pub fn is_dbcs_field(&self, a: usize) -> bool {
        self.field_attr_addr(a)
            .is_some_and(|p| self.cells[p].ext.cs == CS_DBCS)
    }

    /// 表示用のセル（行×桁）。2 バイト文字は 2 セルで 1 文字にする。
    pub fn display(&self, ccsid: Ccsid) -> Vec<DisplayCell> {
        let n = self.len();
        let mut out = vec![DisplayCell::default(); n];
        // 先頭のフィールド属性から始める（最後のフィールドは先頭に折り返す）
        let start = self.field_attrs().first().copied().unwrap_or(0);
        let mut fa: Option<u8> = None;
        let mut field_ext = Ext::default();
        let mut shift = false;
        let mut dbcs_field = false;
        // 最初のフィールドの属性（折り返しの前のフィールド）
        if let Some(p) = self.field_attr_addr(start) {
            fa = self.cells[p].fa;
            field_ext = self.cells[p].ext;
        }
        let mut i = 0;
        while i < n {
            let a = (start + i) % n;
            let c = &self.cells[a];
            if let Some(attr) = c.fa {
                fa = Some(attr);
                field_ext = c.ext;
                shift = false;
                dbcs_field = c.ext.cs == CS_DBCS;
                out[a] = DisplayCell {
                    text: " ".into(),
                    width: 1,
                    fg: field_ext.fg.max(base_color(fa)),
                    bg: field_ext.bg,
                    hl: 0,
                    attribute: true,
                    protected: true,
                    ..DisplayCell::default()
                };
                i += 1;
                continue;
            }
            let hidden = fa.is_some_and(|f| f & FA_DISPLAY_MASK == FA_NONDISPLAY);
            let intensified = fa.is_some_and(|f| f & FA_DISPLAY_MASK == FA_INTENSIFIED);
            let pick = |own: u8, field: u8| if own != 0 { own } else { field };
            let fg = match pick(c.ext.fg, field_ext.fg) {
                0 => base_color(fa),
                v => v,
            };
            let mut cell = DisplayCell {
                text: " ".into(),
                width: 1,
                fg,
                bg: pick(c.ext.bg, field_ext.bg),
                hl: pick(c.ext.hl, field_ext.hl),
                intensified,
                hidden,
                protected: fa.is_some_and(|f| f & FA_PROTECT != 0),
                attribute: false,
            };
            let double = dbcs_field || shift || c.ext.cs == CS_DBCS;
            match c.b {
                FC_SO if !dbcs_field => {
                    shift = true;
                    out[a] = cell;
                    i += 1;
                    continue;
                }
                FC_SI if !dbcs_field => {
                    shift = false;
                    out[a] = cell;
                    i += 1;
                    continue;
                }
                _ => {}
            }
            if double && i + 1 < n {
                let b = (start + i + 1) % n;
                let c2 = &self.cells[b];
                if c2.fa.is_none() && (c2.b != FC_SI || dbcs_field) {
                    let code = u16::from_be_bytes([c.b, c2.b]);
                    let text = match code {
                        0x0000 => None,
                        // 2 バイトの空白
                        0x4040 => Some("\u{3000}".to_owned()),
                        _ => ccsid.decode_double(code),
                    };
                    if let Some(t) = text {
                        cell.text = if hidden { "\u{3000}".into() } else { t };
                        cell.width = 2;
                        let mut second = cell.clone();
                        second.text = String::new();
                        second.width = 0;
                        out[a] = cell;
                        out[b] = second;
                        i += 2;
                        continue;
                    }
                    // 読めない 2 バイト（null の対など）: 2 セルの空白
                    out[a] = cell.clone();
                    out[b] = cell;
                    i += 2;
                    continue;
                }
            }
            if !hidden
                && c.b != FC_NULL
                && let Some(ch) = ccsid.decode_single(c.b)
            {
                cell.text = ch.to_string();
            }
            out[a] = cell;
            i += 1;
        }
        out
    }

    /// 画面の文字（行ごと。非表示のフィールドは空白、行末の空白は除く）。
    pub fn text_lines(&self, ccsid: Ccsid) -> Vec<String> {
        let d = self.display(ccsid);
        d.chunks(self.cols)
            .map(|row| {
                let s: String = row
                    .iter()
                    .map(|c| {
                        if c.hidden || c.attribute {
                            " "
                        } else {
                            c.text.as_str()
                        }
                    })
                    .collect();
                s.trim_end().to_owned()
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen_with_fields() -> Screen {
        let mut s = Screen::new((24, 80), (24, 80));
        // 0: 保護、10: 非保護（5 文字）、16: 保護・数字（自動スキップ）、20: 非保護
        s.cells[0].fa = Some(FA_PROTECT);
        s.cells[10].fa = Some(0);
        s.cells[16].fa = Some(FA_PROTECT | FA_NUMERIC);
        s.cells[20].fa = Some(FA_INTENSIFIED);
        s
    }

    #[test]
    fn finds_fields() {
        let s = screen_with_fields();
        assert!(s.formatted());
        assert_eq!(s.field_attr_addr(12), Some(10));
        assert_eq!(s.field_attr_addr(5), Some(0));
        assert!(s.is_protected(5));
        assert!(!s.is_protected(12));
        assert!(s.is_protected(10));
        assert_eq!(s.field_range(10), (11, 5));
        assert_eq!(s.next_unprotected(0), Some(11));
        assert_eq!(s.next_unprotected(12), Some(21));
        // 折り返す
        assert_eq!(s.next_unprotected(500), Some(11));
        assert_eq!(s.first_unprotected(), 11);
        // 最後のフィールドは先頭に折り返さない（0 の属性で終わる）
        assert_eq!(s.field_range(20), (21, 24 * 80 - 21));
    }

    #[test]
    fn displays_text_colors_and_hidden_fields() {
        let mut s = screen_with_fields();
        for (i, b) in [0xC8, 0xC5, 0xD3, 0xD3, 0xD6].iter().enumerate() {
            s.cells[1 + i].b = *b; // HELLO（保護＝青）
            s.cells[11 + i].b = *b; // 非保護＝緑
        }
        s.cells[30].fa = Some(FA_NONDISPLAY);
        s.cells[31].b = 0xC1;
        let d = s.display(Ccsid::Ibm037);
        let row: String = d[..6].iter().map(|c| c.text.as_str()).collect();
        assert_eq!(row, " HELLO");
        assert_eq!(d[1].fg, 0xF1);
        assert_eq!(d[11].fg, 0xF4);
        assert!(d[31].hidden && d[31].text == " ");
        assert_eq!(s.text_lines(Ccsid::Ibm037)[0], " HELLO     HELLO");
    }

    #[test]
    fn pairs_double_byte_cells() {
        let mut s = Screen::new((24, 80), (24, 80));
        let yy_encoding::EbcdicCode::Double(code) = Ccsid::Ibm930.encode_char('漢').unwrap()
        else {
            panic!()
        };
        let [h, l] = code.to_be_bytes();
        // 書式なし: A SO 漢 SI B
        let bytes = [0xC1, FC_SO, h, l, FC_SI, 0xC2];
        for (i, b) in bytes.iter().enumerate() {
            s.cells[i].b = *b;
        }
        let d = s.display(Ccsid::Ibm930);
        assert_eq!(d[0].text, "A");
        assert_eq!((d[1].text.as_str(), d[1].width), (" ", 1));
        assert_eq!((d[2].text.as_str(), d[2].width), ("漢", 2));
        assert_eq!(d[3].width, 0);
        assert_eq!(d[5].text, "B");
        assert_eq!(s.text_lines(Ccsid::Ibm930)[0], "A 漢 B");
        // DBCS のフィールド: SO/SI なしで 2 セルずつ
        let mut s = Screen::new((24, 80), (24, 80));
        s.cells[0].fa = Some(0);
        s.cells[0].ext.cs = CS_DBCS;
        s.cells[1].b = h;
        s.cells[2].b = l;
        let d = s.display(Ccsid::Ibm930);
        assert_eq!(d[1].text, "漢");
        assert!(s.is_dbcs_field(1));
    }
}
