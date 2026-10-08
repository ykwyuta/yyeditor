//! 共有式（15 章 7.4）: 列の `r0..=r1` 行に、1 行目の式を行ごとに相対的にずらして入れたもの。
//!
//! 5000 万行に 5000 万個の式を持たず、式は 1 つ、結果は列（チャンク）として持つ。計算は
//! [`yy_formula::eval_rows`] でまとめて行う。範囲の中のセルに値や別の式を入れると、そこで分ける。

use std::sync::Arc;

use yy_formula::Edit;

use crate::Context;
use crate::chunk::{Builder, CellRef, Chunk, MAX_ROWS};
use crate::column::{Column, Piece};
use crate::formula::{Formula, from_val};

/// 共有式。
#[derive(Clone, Debug)]
pub struct Shared {
    pub col: u32,
    /// 範囲（絞り込みをしないときの格子の行。両端を含む）
    pub r0: u64,
    pub r1: u64,
    /// 1 行目（`r0`）の式
    pub formula: Formula,
    /// 結果（`r0` からの行の順。まだ計算していなければ `None`）
    pub results: Option<Column>,
}

impl Shared {
    pub fn rows(&self) -> u64 {
        self.r1 - self.r0 + 1
    }

    pub fn contains(&self, row: u64, col: u32) -> bool {
        col == self.col && (self.r0..=self.r1).contains(&row)
    }

    pub fn area(&self) -> yy_formula::Area {
        yy_formula::Area {
            r0: self.r0,
            c0: self.col,
            r1: self.r1,
            c1: self.col,
            ..yy_formula::Area::cell(self.r0, self.col)
        }
    }

    /// `row` 行目の式（相対参照をずらしたもの）。
    pub fn formula_at(&self, row: u64) -> Formula {
        if row == self.r0 {
            return self.formula.clone();
        }
        let e = yy_formula::shift(&self.formula.expr, (row - self.r0) as i64);
        Formula {
            text: Arc::from(yy_formula::formula_text(&e).as_str()),
            expr: Arc::new(e),
        }
    }

    /// `r0..=r1` の部分（1 行目の式をずらす。結果は計算し直す）。
    fn part(&self, r0: u64, r1: u64) -> Shared {
        Shared {
            col: self.col,
            r0,
            r1,
            formula: self.formula_at(r0),
            results: None,
        }
    }

    /// `row` 行を抜いた残り（上と下）。
    pub(crate) fn without(&self, row: u64) -> Vec<Shared> {
        let mut out = Vec::new();
        if row > self.r0 {
            out.push(self.part(self.r0, row - 1));
        }
        if row < self.r1 {
            out.push(self.part(row + 1, self.r1));
        }
        out
    }

    /// 行・列の挿入と削除に合わせる（範囲が消えれば空、挿入が中に入れば 2 つに分かれる）。
    pub(crate) fn edited(&self, edit: Edit) -> Vec<Shared> {
        let keep = |s: Shared| {
            let mut s = s;
            s.results = None;
            vec![s]
        };
        match edit {
            Edit::InsertRows(at, n) => {
                if at <= self.r0 {
                    let mut s = self.clone();
                    s.r0 += n;
                    s.r1 += n;
                    keep(s)
                } else if at > self.r1 {
                    keep(self.clone())
                } else {
                    // 挿入した行は空のまま（Excel と同じ）。下の部分は元の行の式
                    let upper = self.part(self.r0, at - 1);
                    let mut lower = self.part(at, self.r1);
                    lower.r0 += n;
                    lower.r1 += n;
                    vec![upper, lower]
                }
            }
            Edit::DeleteRows(at, n) => {
                let end = at + n;
                let mut out = Vec::new();
                if self.r0 < at {
                    out.push(self.part(self.r0, self.r1.min(at - 1)));
                }
                if self.r1 >= end {
                    let mut lower = self.part(self.r0.max(end), self.r1);
                    lower.r0 -= n;
                    lower.r1 -= n;
                    out.push(lower);
                }
                out
            }
            Edit::InsertCols(at, n) => {
                let mut s = self.clone();
                if s.col >= at {
                    s.col += n;
                }
                keep(s)
            }
            Edit::DeleteCols(at, n) => {
                if (at..at + n).contains(&self.col) {
                    Vec::new()
                } else {
                    let mut s = self.clone();
                    if s.col >= at + n {
                        s.col -= n;
                    }
                    keep(s)
                }
            }
        }
    }
}

/// 結果の値を列（チャンク）にする。
pub(crate) fn results_column(
    ctx: &Context,
    values: impl FnOnce(&mut dyn FnMut(yy_formula::Val)),
) -> std::io::Result<Column> {
    let mut pieces = Vec::new();
    let mut b = Builder::default();
    let mut err = None;
    values(&mut |v| {
        b.push(CellRef::of(&from_val(&v)));
        if b.len() == MAX_ROWS {
            match Chunk::create(ctx, std::mem::take(&mut b).finish()) {
                Ok(c) => pieces.push(Piece {
                    len: c.rows,
                    chunk: c,
                    start: 0,
                }),
                Err(e) => err = Some(e),
            }
        }
    });
    if let Some(e) = err {
        return Err(e);
    }
    if !b.is_empty() {
        let c = Chunk::create(ctx, b.finish())?;
        pieces.push(Piece {
            len: c.rows,
            chunk: c,
            start: 0,
        });
    }
    Ok(Column::from_pieces("", pieces))
}
