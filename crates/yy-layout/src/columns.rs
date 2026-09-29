//! 表示桁（09 章 2.2）。
//!
//! 矩形選択の左右端は、バイトや文字数ではなく「表示桁」で定義する。
//! 全角文字は 2 桁、タブは次のタブ位置まで、不正バイト（`\xNN` 表示）は 4 桁。
//! 結合文字など幅 0 の文字は直前の文字と一体として扱い、その間では区切らない。

use unicode_width::UnicodeWidthChar;

use crate::row::{Row, SpanKind};

/// 桁の数え方。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColumnConfig {
    pub tab_width: u32,
    /// 東アジアの曖昧幅文字（①、○、ギリシャ文字など）を 2 桁として数えるか
    pub ambiguous_wide: bool,
}

impl Default for ColumnConfig {
    fn default() -> Self {
        ColumnConfig {
            tab_width: 4,
            ambiguous_wide: true,
        }
    }
}

impl ColumnConfig {
    /// 文字 `c` を桁 `col` に置いたときの幅。
    pub fn char_width(&self, c: char, col: u32) -> u32 {
        match c {
            '\t' => {
                let t = self.tab_width.max(1);
                t - col % t
            }
            c => {
                let w = if self.ambiguous_wide {
                    c.width_cjk()
                } else {
                    c.width()
                };
                w.unwrap_or(1) as u32
            }
        }
    }

    /// 文字列を桁 `start_col` から置いたときの幅の合計。
    pub fn text_width(&self, text: &str, start_col: u32) -> u32 {
        let mut col = start_col;
        for c in text.chars() {
            col += self.char_width(c, col);
        }
        col - start_col
    }
}

/// 表示行の中の 1 単位（区切ってよい最小単位）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unit {
    pub start: u64,
    pub end: u64,
    pub col_start: u32,
    pub col_end: u32,
}

/// 表示行を単位に分解する。
pub fn units(row: &Row, cfg: &ColumnConfig) -> Vec<Unit> {
    let mut out: Vec<Unit> = Vec::with_capacity(row.text.len());
    let mut col = 0u32;
    for span in &row.spans {
        let src0 = row.start + span.src.start as u64;
        match span.kind {
            SpanKind::Text => {
                for (i, c) in row.text[span.range.clone()].char_indices() {
                    let start = src0 + i as u64;
                    let end = start + c.len_utf8() as u64;
                    let w = cfg.char_width(c, col);
                    if w == 0
                        && let Some(last) = out.last_mut()
                        && last.end == start
                    {
                        // 結合文字などは直前の文字に含める
                        last.end = end;
                        continue;
                    }
                    out.push(Unit {
                        start,
                        end,
                        col_start: col,
                        col_end: col + w,
                    });
                    col += w;
                }
            }
            SpanKind::Control | SpanKind::Invalid => {
                let w = if span.kind == SpanKind::Invalid { 4 } else { 1 };
                for b in span.src.clone() {
                    let start = row.start + b as u64;
                    out.push(Unit {
                        start,
                        end: start + 1,
                        col_start: col,
                        col_end: col + w,
                    });
                    col += w;
                }
            }
        }
    }
    out
}

/// 行の内容（改行を除く）の桁数。
pub fn content_cols(units: &[Unit]) -> u32 {
    units.last().map_or(0, |u| u.col_end)
}

/// 位置 `offset`（行の外は端に丸める）の桁。
pub fn col_of(units: &[Unit], row: &Row, offset: u64) -> u32 {
    let offset = offset.clamp(row.start, row.end);
    let mut col = 0;
    for u in units {
        if u.end <= offset {
            col = u.col_end;
        } else {
            break;
        }
    }
    col
}

/// 桁の途中にかかる文字の扱い。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    /// 矩形の左端: かかった文字を含める（文字の前で区切る）
    Left,
    /// 矩形の右端: かかった文字を含める（文字の後で区切る）
    Right,
    /// マウス位置など: 近い方の境界
    Nearest,
}

/// 桁 `col` に対応する位置と、その位置の実際の桁。行末より右なら行末を返す。
pub fn offset_at_col(units: &[Unit], row: &Row, col: u32, edge: Edge) -> (u64, u32) {
    let mut pos = (row.start, 0);
    for u in units {
        if u.col_end <= col {
            pos = (u.end, u.col_end);
            continue;
        }
        if u.col_start >= col {
            return (u.start, u.col_start);
        }
        // 文字の途中にかかっている
        let take_after = match edge {
            Edge::Left => false,
            Edge::Right => true,
            Edge::Nearest => (col - u.col_start) * 2 >= (u.col_end - u.col_start),
        };
        return if take_after {
            (u.end, u.col_end)
        } else {
            (u.start, u.col_start)
        };
    }
    if units.is_empty() {
        (row.start, 0)
    } else {
        (row.end.max(pos.0), pos.1)
    }
}
