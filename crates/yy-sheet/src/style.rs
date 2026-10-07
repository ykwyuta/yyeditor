//! セルの書式（15 章 11）。
//!
//! 5000 万行のセルに個別の書式を持たないよう、書式は**範囲で持つ**: 長方形の範囲（列全体・行全体を
//! 含む）と書式の差分の組を付けた順に重ね、後から付けたものが優先する。「書式のクリア」は範囲の書式を
//! 既定に戻す層として重ねる。
//!
//! 行は絞り込み・並べ替えをしないときの格子の行（[`crate::Sheet::source_row`]）で数える。

use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// 色（0xRRGGBB）。
pub type Rgb = u32;

/// 「色なし」（塗りつぶしなし・文字は自動の色）を明示する値。
pub const NO_COLOR: Rgb = u32::MAX;

/// 横の配置。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HAlign {
    /// 値の種類で決める（数値は右、文字列は左）
    General,
    Left,
    Center,
    Right,
}

/// 罫線の種類。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LineStyle {
    /// 線なし（下の層の線を消す）
    None,
    Thin,
    Medium,
    Thick,
    Dotted,
    Dashed,
    Double,
}

impl LineStyle {
    /// 重なったときの優先度（Excel と同じく太い線・二重線が勝つ）。
    pub fn weight(self) -> u8 {
        match self {
            LineStyle::None => 0,
            LineStyle::Dotted => 1,
            LineStyle::Dashed => 2,
            LineStyle::Thin => 3,
            LineStyle::Medium => 4,
            LineStyle::Double => 5,
            LineStyle::Thick => 6,
        }
    }
}

/// 罫線。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Line {
    pub style: LineStyle,
    pub color: Rgb,
}

/// 辺（上・右・下・左）。
pub const TOP: usize = 0;
pub const RIGHT: usize = 1;
pub const BOTTOM: usize = 2;
pub const LEFT: usize = 3;

/// 書式（差分）。`None` の項目は下の層（なければ既定）のまま。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Style {
    /// 表示形式（Excel の書式記号）
    pub num_fmt: Option<Arc<str>>,
    /// 塗りつぶし（[`NO_COLOR`] でなし）
    pub fill: Option<Rgb>,
    /// 文字の色（[`NO_COLOR`] で自動）
    pub color: Option<Rgb>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub align: Option<HAlign>,
    /// 罫線（上・右・下・左）
    pub border: [Option<Line>; 4],
}

impl Style {
    pub fn is_empty(&self) -> bool {
        *self == Style::default()
    }

    /// `o` の項目（`Some` のもの）で上書きする。
    pub fn apply(&mut self, o: &Style) {
        if o.num_fmt.is_some() {
            self.num_fmt = o.num_fmt.clone();
        }
        macro_rules! set {
            ($($f:ident),*) => {$(if o.$f.is_some() { self.$f = o.$f; })*};
        }
        set!(fill, color, bold, italic, align);
        for i in 0..4 {
            if o.border[i].is_some() {
                self.border[i] = o.border[i];
            }
        }
    }

    /// 塗りつぶしの色（なければ `None`）。
    pub fn fill_rgb(&self) -> Option<Rgb> {
        self.fill.filter(|&c| c != NO_COLOR)
    }

    /// 文字の色（自動なら `None`）。
    pub fn color_rgb(&self) -> Option<Rgb> {
        self.color.filter(|&c| c != NO_COLOR)
    }

    /// 辺の線（線なしなら `None`）。
    pub fn line(&self, side: usize) -> Option<Line> {
        self.border[side].filter(|l| l.style != LineStyle::None)
    }
}

/// 長方形の範囲（両端を含む。`u64::MAX`・`u32::MAX` まで伸ばせば列全体・行全体）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub top: u64,
    pub left: u32,
    pub bottom: u64,
    pub right: u32,
}

impl Rect {
    pub fn new(top: u64, left: u32, bottom: u64, right: u32) -> Rect {
        Rect {
            top: top.min(bottom),
            left: left.min(right),
            bottom: top.max(bottom),
            right: left.max(right),
        }
    }

    pub fn contains(&self, row: u64, col: u32) -> bool {
        (self.top..=self.bottom).contains(&row) && (self.left..=self.right).contains(&col)
    }

    /// 列全体か。
    pub fn whole_cols(&self) -> bool {
        self.top == 0 && self.bottom == u64::MAX
    }
}

/// 書式の層。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    pub rect: Rect,
    /// 先に範囲の書式を既定に戻す（書式のクリア）
    pub clear: bool,
    pub style: Style,
}

/// 層の数がこれを超えたら、覆われて効かなくなった層を捨てる。
const COMPACT_AT: usize = 4096;

/// シートの書式（層の並び）。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Styles {
    layers: Arc<Vec<Layer>>,
}

impl Styles {
    pub fn layers(&self) -> &[Layer] {
        &self.layers
    }

    pub fn from_layers(layers: Vec<Layer>) -> Styles {
        Styles {
            layers: Arc::new(layers),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.layers.is_empty()
    }

    /// セルの書式（すべての層を重ねたもの）。
    pub fn at(&self, row: u64, col: u32) -> Style {
        let mut s = Style::default();
        for l in self.layers.iter() {
            if l.rect.contains(row, col) {
                if l.clear {
                    s = Style::default();
                }
                s.apply(&l.style);
            }
        }
        s
    }

    /// 範囲に書式を付ける（罫線の「外枠」は呼ぶ側で辺ごとの範囲に分ける）。
    pub fn set(&mut self, rect: Rect, style: Style) {
        if style.is_empty() {
            return;
        }
        self.push(Layer {
            rect,
            clear: false,
            style,
        });
    }

    /// 範囲の書式を既定に戻す。
    pub fn clear(&mut self, rect: Rect) {
        // 範囲にすっぽり入る層は捨てるだけでよい
        let inside = |r: &Rect| {
            r.top >= rect.top
                && r.bottom <= rect.bottom
                && r.left >= rect.left
                && r.right <= rect.right
        };
        let layers = Arc::make_mut(&mut self.layers);
        layers.retain(|l| !inside(&l.rect));
        if layers.iter().any(|l| overlaps(&l.rect, &rect)) {
            layers.push(Layer {
                rect,
                clear: true,
                style: Style::default(),
            });
        }
    }

    fn push(&mut self, layer: Layer) {
        let layers = Arc::make_mut(&mut self.layers);
        // 同じ範囲への続けての書式は 1 つにまとめる
        if let Some(last) = layers.last_mut()
            && last.rect == layer.rect
        {
            last.style.apply(&layer.style);
            return;
        }
        layers.push(layer);
        if layers.len() > COMPACT_AT {
            compact(layers);
        }
    }

    /// 行の挿入（`at` 行目の前に `n` 行）。
    pub fn insert_rows(&mut self, at: u64, n: u64) {
        self.map(|r| {
            let mut r = *r;
            if r.top >= at {
                r.top = r.top.saturating_add(n);
            }
            if r.bottom >= at && r.bottom != u64::MAX {
                r.bottom = r.bottom.saturating_add(n);
            }
            Some(r)
        });
    }

    /// 行の削除（`at` 行目から `n` 行）。
    pub fn delete_rows(&mut self, at: u64, n: u64) {
        let end = at + n;
        self.map(|r| {
            let mut r = *r;
            let shrink = |x: u64| {
                if x == u64::MAX || x < at {
                    x
                } else if x >= end {
                    x - n
                } else {
                    at
                }
            };
            if r.top >= at && r.bottom < end {
                return None;
            }
            let bottom_inside = r.bottom >= at && r.bottom < end;
            r.top = shrink(r.top);
            r.bottom = if bottom_inside {
                at.checked_sub(1)?
            } else {
                shrink(r.bottom)
            };
            (r.top <= r.bottom).then_some(r)
        });
    }

    /// 列の挿入。
    pub fn insert_cols(&mut self, at: u32, n: u32) {
        self.map(|r| {
            let mut r = *r;
            if r.left >= at {
                r.left = r.left.saturating_add(n);
            }
            if r.right >= at && r.right != u32::MAX {
                r.right = r.right.saturating_add(n);
            }
            Some(r)
        });
    }

    /// 列の削除。
    pub fn delete_cols(&mut self, at: u32, n: u32) {
        let end = at + n;
        self.map(|r| {
            let mut r = *r;
            let shrink = |x: u32| {
                if x == u32::MAX || x < at {
                    x
                } else if x >= end {
                    x - n
                } else {
                    at
                }
            };
            if r.left >= at && r.right < end {
                return None;
            }
            let right_inside = r.right >= at && r.right < end;
            r.left = shrink(r.left);
            r.right = if right_inside {
                at.checked_sub(1)?
            } else {
                shrink(r.right)
            };
            (r.left <= r.right).then_some(r)
        });
    }

    fn map(&mut self, f: impl Fn(&Rect) -> Option<Rect>) {
        if self.layers.is_empty() {
            return;
        }
        let layers = Arc::make_mut(&mut self.layers);
        layers.retain_mut(|l| match f(&l.rect) {
            Some(r) => {
                l.rect = r;
                true
            }
            None => false,
        });
    }
}

/// 罫線の付け方。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BorderPreset {
    /// 罫線なし（範囲の 4 辺とも消す）
    None,
    /// 格子（すべてのセルの 4 辺）
    All,
    /// 外枠
    Outline,
    Top,
    Bottom,
    Left,
    Right,
}

/// 範囲に罫線を付ける書式の層（範囲・書式）。
pub fn border_layers(rect: Rect, preset: BorderPreset, line: Line) -> Vec<(Rect, Style)> {
    let side = |sides: &[usize], l: Line| {
        let mut st = Style::default();
        for &i in sides {
            st.border[i] = Some(l);
        }
        st
    };
    let none = Line {
        style: LineStyle::None,
        color: 0,
    };
    let top_row = Rect {
        bottom: rect.top,
        ..rect
    };
    let bottom_row = Rect {
        top: rect.bottom,
        ..rect
    };
    let left_col = Rect {
        right: rect.left,
        ..rect
    };
    let right_col = Rect {
        left: rect.right,
        ..rect
    };
    match preset {
        BorderPreset::None => vec![(rect, side(&[TOP, RIGHT, BOTTOM, LEFT], none))],
        BorderPreset::All => vec![(rect, side(&[TOP, RIGHT, BOTTOM, LEFT], line))],
        BorderPreset::Outline => vec![
            (top_row, side(&[TOP], line)),
            (bottom_row, side(&[BOTTOM], line)),
            (left_col, side(&[LEFT], line)),
            (right_col, side(&[RIGHT], line)),
        ],
        BorderPreset::Top => vec![(top_row, side(&[TOP], line))],
        BorderPreset::Bottom => vec![(bottom_row, side(&[BOTTOM], line))],
        BorderPreset::Left => vec![(left_col, side(&[LEFT], line))],
        BorderPreset::Right => vec![(right_col, side(&[RIGHT], line))],
    }
}

fn overlaps(a: &Rect, b: &Rect) -> bool {
    a.top <= b.bottom && b.top <= a.bottom && a.left <= b.right && b.left <= a.right
}

/// あとの「クリア」の層にすっぽり覆われた層を捨てる。
fn compact(layers: &mut Vec<Layer>) {
    let mut keep = vec![true; layers.len()];
    for (i, l) in layers.iter().enumerate() {
        if l.clear {
            for (j, k) in keep.iter_mut().enumerate().take(i) {
                let r = &layers[j].rect;
                if r.top >= l.rect.top
                    && r.bottom <= l.rect.bottom
                    && r.left >= l.rect.left
                    && r.right <= l.rect.right
                {
                    *k = false;
                }
            }
        }
    }
    let mut i = 0;
    layers.retain(|_| {
        i += 1;
        keep[i - 1]
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fill(c: Rgb) -> Style {
        Style {
            fill: Some(c),
            ..Style::default()
        }
    }

    #[test]
    fn layers_override_in_order() {
        let mut s = Styles::default();
        s.set(Rect::new(0, 1, u64::MAX, 1), fill(0xFF0000));
        s.set(
            Rect::new(2, 0, 3, 5),
            Style {
                bold: Some(true),
                fill: Some(0x00FF00),
                ..Style::default()
            },
        );
        assert_eq!(s.at(0, 1).fill_rgb(), Some(0xFF0000));
        assert_eq!(s.at(2, 1).fill_rgb(), Some(0x00FF00));
        assert_eq!(s.at(2, 4).bold, Some(true));
        assert_eq!(s.at(9, 4), Style::default());
        // 同じ範囲は 1 つの層にまとめる
        s.set(
            Rect::new(2, 0, 3, 5),
            Style {
                italic: Some(true),
                ..Style::default()
            },
        );
        assert_eq!(s.layers().len(), 2);
        assert_eq!(s.at(3, 0).italic, Some(true));
        assert_eq!(s.at(3, 0).bold, Some(true));
        // クリア: 中に入る層は捨て、はみ出す層の上には既定に戻す層を重ねる
        s.clear(Rect::new(0, 0, 10, 5));
        assert_eq!(s.layers().len(), 2);
        assert_eq!(s.at(2, 1), Style::default());
        assert_eq!(s.at(20, 1).fill_rgb(), Some(0xFF0000));
        // 色なしで上書き
        s.set(Rect::new(20, 1, 20, 1), fill(NO_COLOR));
        assert_eq!(s.at(20, 1).fill_rgb(), None);
    }

    #[test]
    fn border_presets() {
        let line = Line {
            style: LineStyle::Thin,
            color: 0x112233,
        };
        let mut s = Styles::default();
        for (r, st) in border_layers(Rect::new(1, 1, 3, 2), BorderPreset::Outline, line) {
            s.set(r, st);
        }
        assert_eq!(s.at(1, 1).line(TOP), Some(line));
        assert_eq!(s.at(1, 1).line(LEFT), Some(line));
        assert_eq!(s.at(1, 1).line(RIGHT), None);
        assert_eq!(s.at(2, 1).line(TOP), None);
        assert_eq!(s.at(3, 2).line(BOTTOM), Some(line));
        assert_eq!(s.at(3, 2).line(RIGHT), Some(line));
        for (r, st) in border_layers(Rect::new(1, 1, 3, 2), BorderPreset::None, line) {
            s.set(r, st);
        }
        assert!((0..4).all(|i| s.at(3, 2).line(i).is_none()));
    }

    #[test]
    fn rows_and_cols_follow_edits() {
        let mut s = Styles::default();
        s.set(Rect::new(5, 2, 9, 3), fill(1));
        s.set(Rect::new(0, 4, u64::MAX, 4), fill(2));
        s.insert_rows(6, 2);
        assert_eq!(s.layers()[0].rect, Rect::new(5, 2, 11, 3));
        assert_eq!(s.layers()[1].rect, Rect::new(0, 4, u64::MAX, 4));
        s.delete_rows(4, 3);
        assert_eq!(s.layers()[0].rect, Rect::new(4, 2, 8, 3));
        s.delete_rows(4, 5);
        assert_eq!(s.layers().len(), 1);
        s.insert_cols(0, 1);
        assert_eq!(s.layers()[0].rect.left, 5);
        s.delete_cols(5, 1);
        assert!(s.is_empty());
        // 一部を削れば縮む
        s.set(Rect::new(0, 2, 0, 6), fill(3));
        s.delete_cols(4, 5);
        assert_eq!(s.layers()[0].rect, Rect::new(0, 2, 0, 3));
    }
}
