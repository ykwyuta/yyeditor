//! 矩形選択（09 章 2〜3）。
//!
//! 矩形は（先頭の表示行・末尾の表示行・左右の表示桁）で表し、行ごとの範囲は必要になったとき
//! だけ計算する（遅延展開）。描画では表示中の行だけを、編集では全行を展開する。

use std::ops::Range;

use yy_buffer::Snapshot;

use crate::columns::{ColumnConfig, Edge, Unit, col_of, content_cols, offset_at_col, units};
use crate::{RowConfig, next_row_start, row_at, row_containing};

/// 矩形選択。行は表示行の開始位置で、桁は表示桁で表す。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RectSelection {
    pub anchor_row: u64,
    pub anchor_col: u32,
    pub head_row: u64,
    pub head_col: u32,
}

impl RectSelection {
    pub fn top(&self) -> u64 {
        self.anchor_row.min(self.head_row)
    }

    pub fn bottom(&self) -> u64 {
        self.anchor_row.max(self.head_row)
    }

    pub fn left(&self) -> u32 {
        self.anchor_col.min(self.head_col)
    }

    pub fn right(&self) -> u32 {
        self.anchor_col.max(self.head_col)
    }

    /// 幅 0（縦一列のカーソル）か。
    pub fn is_zero_width(&self) -> bool {
        self.anchor_col == self.head_col
    }

    /// 幅 0 の矩形（縦一列のカーソル）にする。
    pub fn collapse_to(&self, col: u32) -> RectSelection {
        RectSelection {
            anchor_col: col,
            head_col: col,
            ..*self
        }
    }
}

/// 矩形の 1 行分。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RectRow {
    pub row_start: u64,
    /// 選択範囲（行の内容の中）
    pub range: Range<u64>,
    pub start_col: u32,
    pub end_col: u32,
    /// 行末より右の仮想空白に入力するときに補う空白の数
    pub pad: u32,
    pub units: Vec<Unit>,
}

/// 矩形の行数が上限を超えた。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TooManyRows(pub usize);

/// 表示行 `row_start` の矩形部分を計算する。
pub fn rect_row(
    snap: &Snapshot,
    rcfg: RowConfig,
    ccfg: &ColumnConfig,
    rect: &RectSelection,
    row_start: u64,
) -> RectRow {
    let row = row_at(snap, rcfg, row_start);
    let us = units(&row, ccfg);
    let content = content_cols(&us);
    let (left, right) = (rect.left(), rect.right());
    let (start, start_col) = offset_at_col(&us, &row, left, Edge::Left);
    let (end, end_col) = if rect.is_zero_width() {
        (start, start_col)
    } else {
        offset_at_col(&us, &row, right, Edge::Right)
    };
    RectRow {
        row_start,
        range: start..end.max(start),
        start_col,
        end_col,
        pad: left.saturating_sub(content),
        units: us,
    }
}

/// 矩形のすべての行を上から順に展開する。`limit` 行を超える場合はエラー。
pub fn rect_rows(
    snap: &Snapshot,
    rcfg: RowConfig,
    ccfg: &ColumnConfig,
    rect: &RectSelection,
    limit: usize,
) -> Result<Vec<RectRow>, TooManyRows> {
    let bottom = rect.bottom();
    let mut out = Vec::new();
    let mut r = rect.top();
    loop {
        if out.len() >= limit {
            return Err(TooManyRows(limit));
        }
        out.push(rect_row(snap, rcfg, ccfg, rect, r));
        if r >= bottom {
            break;
        }
        match next_row_start(snap, rcfg, r) {
            Some(n) if n <= bottom => r = n,
            _ => break,
        }
    }
    Ok(out)
}

/// 矩形に対する編集: 置き換える範囲と挿入するバイト列の列（昇順）と、編集後の矩形の桁。
pub struct RectEdit {
    pub changes: Vec<(Range<u64>, Vec<u8>)>,
    pub new_col: u32,
}

/// 各行の矩形部分を `texts[i]`（行数より少なければ最後の値を繰り返す）で置き換える。
/// 行末より右に入力する場合は空白で埋める。
pub fn replace_rows(rows: &[RectRow], texts: &[&str], ccfg: &ColumnConfig) -> RectEdit {
    let mut changes = Vec::with_capacity(rows.len());
    let mut new_col = rows.first().map_or(0, |r| r.start_col + r.pad);
    for (i, r) in rows.iter().enumerate() {
        let text = texts.get(i).or(texts.last()).copied().unwrap_or("");
        let mut bytes = vec![b' '; r.pad as usize];
        bytes.extend_from_slice(text.as_bytes());
        if i == 0 {
            new_col = r.start_col + r.pad + ccfg.text_width(text, r.start_col + r.pad);
        }
        changes.push((r.range.clone(), bytes));
    }
    RectEdit { changes, new_col }
}

/// BackSpace: 幅 0 の矩形ではカーソルの左の 1 文字を各行で削除する。
/// 幅のある矩形では選択部分を削除する。
pub fn delete_backward(rows: &[RectRow], rect: &RectSelection) -> RectEdit {
    if !rect.is_zero_width() {
        return RectEdit {
            changes: rows.iter().map(|r| (r.range.clone(), Vec::new())).collect(),
            new_col: rect.left(),
        };
    }
    let col = rect.left();
    if col == 0 {
        return RectEdit {
            changes: Vec::new(),
            new_col: 0,
        };
    }
    let mut changes = Vec::new();
    // 新しい桁は先頭行に合わせる（全角と半角が混在すると全行を揃えることはできない）
    let mut new_col = None;
    for r in rows {
        if r.pad > 0 {
            // 仮想空白の中では文字を消さずにカーソルだけ左へ動く
            new_col.get_or_insert(col - 1);
            continue;
        }
        if let Some(u) = r.units.iter().rev().find(|u| u.col_end <= col) {
            changes.push((u.start..u.end, Vec::new()));
            new_col.get_or_insert(u.col_start);
        }
    }
    RectEdit {
        changes,
        new_col: new_col.unwrap_or(col - 1),
    }
}

/// Delete: 幅 0 の矩形ではカーソルの右の 1 文字を各行で削除する。
pub fn delete_forward(rows: &[RectRow], rect: &RectSelection) -> RectEdit {
    if !rect.is_zero_width() {
        return delete_backward(rows, rect);
    }
    let col = rect.left();
    let changes = rows
        .iter()
        .filter_map(|r| r.units.iter().find(|u| u.col_start >= col))
        .map(|u| (u.start..u.end, Vec::new()))
        .collect();
    RectEdit {
        changes,
        new_col: col,
    }
}

/// 矩形部分のテキストを行ごとに取り出す（コピー用）。
pub fn row_texts(snap: &Snapshot, rows: &[RectRow]) -> Vec<Vec<u8>> {
    rows.iter().map(|r| snap.read(r.range.clone())).collect()
}

/// 位置 `offset` を含む表示行と、その位置の桁。
pub fn row_and_col(
    snap: &Snapshot,
    rcfg: RowConfig,
    ccfg: &ColumnConfig,
    offset: u64,
) -> (u64, u32) {
    let rs = row_containing(snap, rcfg, offset);
    let row = row_at(snap, rcfg, rs);
    let us = units(&row, ccfg);
    (rs, col_of(&us, &row, offset))
}

/// 表示行 `row_start` の桁 `col` に最も近い位置（仮想空白は行末に丸める）。
pub fn offset_at(
    snap: &Snapshot,
    rcfg: RowConfig,
    ccfg: &ColumnConfig,
    row_start: u64,
    col: u32,
) -> u64 {
    let row = row_at(snap, rcfg, row_start);
    let us = units(&row, ccfg);
    offset_at_col(&us, &row, col, Edge::Nearest).0
}
