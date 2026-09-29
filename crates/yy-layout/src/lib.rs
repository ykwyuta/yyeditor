//! 表示用レイアウト計算（描画 API 非依存）。
//!
//! 文書を「表示行（row）」に分割する。通常は 1 論理行 = 1 表示行だが、
//! 数 GB の 1 行のような長大行でも表示・スクロールが破綻しないよう、
//! 長い行は最大 `max_row_bytes` 程度の表示セグメントに分割する（02 章 3.5）。
//!
//! # 分割規則
//!
//! 表示行の開始位置は次の和集合とする（`S = max_row_bytes`）。
//!
//! * 論理行の先頭
//! * 格子点 `k * S`（k ≥ 1、UTF-8 の文字境界まで前方に調整）のうち、
//!   その位置を含む論理行の先頭から `S` 以上離れているもの
//!
//! この規則により、前方・後方のどちらからでも高々 `2S` バイトを調べるだけで
//! 隣の表示行の開始位置が決まり、スクロールのコストがファイルサイズや行の長さに依存しない。
//! 長さ `S` 以下の行は分割されない。

pub mod columns;
pub mod rect;
mod row;
mod viewport;

pub use columns::ColumnConfig;
pub use rect::RectSelection;
pub use row::{Row, Span, SpanKind, decode_row};
pub use viewport::Viewport;

use yy_buffer::{LinePosition, Snapshot};

/// 表示行の分割設定。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowConfig {
    pub max_row_bytes: u64,
}

impl Default for RowConfig {
    fn default() -> Self {
        RowConfig {
            max_row_bytes: 8192,
        }
    }
}

impl RowConfig {
    pub fn new(max_row_bytes: u64) -> RowConfig {
        assert!(max_row_bytes >= 8, "max_row_bytes too small");
        RowConfig { max_row_bytes }
    }
}

/// `offset` が論理行の先頭か。
pub fn is_line_start(snap: &Snapshot, offset: u64) -> bool {
    offset == 0 || snap.byte_at(offset - 1) == Some(b'\n')
}

/// 格子点 `g` を UTF-8 の文字境界まで前方に進める（最大 3 バイト、文書末で止まる）。
fn align_grid(snap: &Snapshot, g: u64) -> u64 {
    let len = snap.len();
    if g >= len {
        return g;
    }
    let bytes = snap.read(g..(g + 4).min(len));
    g + yy_buffer::align_utf8_forward(&bytes, 0) as u64
}

/// 表示行 `row` の次の表示行の開始位置。`row` が最後の表示行なら `None`。
///
/// 文書が改行で終わる場合、文書末（`len`）は空の最終行の開始位置になる。
pub fn next_row_start(snap: &Snapshot, cfg: RowConfig, row: u64) -> Option<u64> {
    let len = snap.len();
    if row >= len {
        return None;
    }
    let s = cfg.max_row_bytes;
    let raw_next = if is_line_start(snap, row) {
        (row + s).div_ceil(s) * s
    } else {
        (row / s + 1) * s
    };
    let grid = align_grid(snap, raw_next);
    if let Some(nl) = snap.find_next(row..grid.min(len), b'\n') {
        return Some(nl + 1);
    }
    (grid < len).then_some(grid)
}

/// 位置 `pos` より前（`pos` を含まない）で最も後ろの表示行の開始位置。
pub fn prev_row_start(snap: &Snapshot, cfg: RowConfig, pos: u64) -> Option<u64> {
    let pos = pos.min(snap.len() + 1);
    if pos == 0 {
        return None;
    }
    let s = cfg.max_row_bytes;
    // pos-1 を含む論理行の先頭を 2S バイト（＋文字境界調整の余裕）以内で探す
    let win_start = pos.saturating_sub(2 * s + 4);
    let line_start = if pos >= 2 {
        match snap.find_prev(win_start..pos - 1, b'\n') {
            Some(n) => Some(n + 1),
            None if win_start == 0 => Some(0),
            None => None,
        }
    } else {
        Some(0)
    };
    // pos より前で最も後ろの格子点（k ≥ 1）
    let mut k = (pos - 1) / s;
    let grid = loop {
        if k == 0 {
            break None;
        }
        let g = align_grid(snap, k * s);
        if g < pos {
            break Some((k * s, g));
        }
        k -= 1;
    };
    Some(match line_start {
        Some(ls) => match grid {
            Some((raw, g)) if raw >= ls + s => g,
            _ => ls,
        },
        // 2S 以内に改行がない ⇒ 論理行の先頭は十分遠く、直前の格子点が表示行の先頭
        None => grid.expect("grid point must exist within 2S").1,
    })
}

/// `offset` を含む表示行の開始位置。
pub fn row_containing(snap: &Snapshot, cfg: RowConfig, offset: u64) -> u64 {
    let len = snap.len();
    if offset >= len {
        if is_line_start(snap, len) {
            return len;
        }
        return prev_row_start(snap, cfg, len).unwrap_or(0);
    }
    prev_row_start(snap, cfg, offset + 1).unwrap_or(0)
}

/// 表示行 `start` の内容を取り出す。
pub fn row_at(snap: &Snapshot, cfg: RowConfig, start: u64) -> Row {
    let len = snap.len();
    let next = next_row_start(snap, cfg, start).unwrap_or(len);
    let bytes = snap.read(start..next);
    Row::new(start, next, is_line_start(snap, start), &bytes)
}

/// `start` から最大 `count` 個の表示行を取り出す。
pub fn rows_from(snap: &Snapshot, cfg: RowConfig, start: u64, count: usize) -> Vec<Row> {
    let mut rows = Vec::with_capacity(count.min(1024));
    let mut pos = Some(start);
    while rows.len() < count {
        let Some(p) = pos else { break };
        if p > snap.len() || (p == snap.len() && !is_line_start(snap, p)) {
            break;
        }
        let row = row_at(snap, cfg, p);
        pos = next_row_start(snap, cfg, p);
        rows.push(row);
    }
    rows
}

/// 表示行 `row_start` の論理行番号（0 始まり）。
pub fn line_number(snap: &Snapshot, row_start: u64) -> LinePosition {
    snap.line_of_offset(row_start)
}
