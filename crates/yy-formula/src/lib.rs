//! 数式（15 章 7）: Excel と同じ文法の解析・評価と、`XLOOKUP`・`SUMIFS`・`COUNTIFS`・`SUM`・`COUNT`・
//! `AVERAGE`・`MAX`・`MIN`・`MEDIAN`・`PERCENTILE`・`ROUND`・`ROUNDUP`・`ROUNDDOWN`・`IF`・`MOD`・`PI`・`TRUE`・`FALSE`・`PRODUCT`・`ABS`・`CONCAT`・
//! `TEXTJOIN`・`TEXTSPLIT`・`LOW-VALUE`・`HIGH-VALUE`・四則演算・文字列の連結。
//!
//! セルの保管には依存しない: 評価はセルの読み方（[`Grid`]）を受け取って行う。列をまとめて読む
//! [`Grid::scan`] を使うので、5000 万行の列に対する `SUMIFS` も列を 1 回読むだけで済む。

use std::fmt;
use std::ops::Range;
use std::sync::Arc;

mod adjust;
mod assist;
mod criteria;
mod eval;
mod func;
mod index;
mod parse;
mod print;
mod rows;
#[cfg(test)]
mod tests;

pub use adjust::{Edit, adjust};
pub use assist::{
    FUNCTIONS, FuncInfo, RefSpan, Typing, area_text, refs_in, sheet_prefix, toggle_abs, typing,
};
pub use eval::{Context, eval};
pub use index::{Cache, ExactIndex};
pub use parse::{Area, AreaKind, BinOp, Expr, Func, ParseError, Ref, Rounding, parse};
pub use print::formula_text;
pub use rows::{eval_rows, shift, shift_by, spread};

/// `LOW-VALUE()` の値（印の文字列。固定長の項目の列では項目をすべて X'00' にし、そのほかの列では
/// `#VALUE!` にする。比べると `LOW-VALUE()` どうしは等しい）。
pub const LOW_VALUE: &str = "\u{F8F0}LOW-VALUE";
/// `HIGH-VALUE()` の値（項目をすべて X'FF' にする）。
pub const HIGH_VALUE: &str = "\u{F8F0}HIGH-VALUE";

/// 表意定数（`LOW-VALUE`・`HIGH-VALUE`）の印の文字列なら、項目を埋めるバイト。
pub fn figurative(s: &str) -> Option<u8> {
    match s {
        LOW_VALUE => Some(0x00),
        HIGH_VALUE => Some(0xFF),
        _ => None,
    }
}

/// エラー値（Excel と同じ。並びは `yy_sheet::CellError` と同じ）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Error {
    Null,
    Div0,
    Value,
    Ref,
    Name,
    Num,
    NA,
    Spill,
    Calc,
}

impl Error {
    pub const ALL: [Error; 9] = [
        Error::Null,
        Error::Div0,
        Error::Value,
        Error::Ref,
        Error::Name,
        Error::Num,
        Error::NA,
        Error::Spill,
        Error::Calc,
    ];

    pub fn text(self) -> &'static str {
        match self {
            Error::Null => "#NULL!",
            Error::Div0 => "#DIV/0!",
            Error::Value => "#VALUE!",
            Error::Ref => "#REF!",
            Error::Name => "#NAME?",
            Error::Num => "#NUM!",
            Error::NA => "#N/A",
            Error::Spill => "#SPILL!",
            Error::Calc => "#CALC!",
        }
    }

    pub fn parse(s: &str) -> Option<Error> {
        Error::ALL
            .into_iter()
            .find(|e| e.text().eq_ignore_ascii_case(s.trim()))
    }

    pub fn code(self) -> u8 {
        self as u8
    }

    pub fn from_code(c: u8) -> Option<Error> {
        Error::ALL.get(c as usize).copied()
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.text())
    }
}

/// 値。
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Val {
    #[default]
    Empty,
    Num(f64),
    Text(Arc<str>),
    Bool(bool),
    Err(Error),
    /// 複数の値（スピルする）
    Array(Arc<Array>),
}

impl Val {
    pub fn text(s: &str) -> Val {
        Val::Text(Arc::from(s))
    }

    pub fn of(c: Cell<'_>) -> Val {
        match c {
            Cell::Empty => Val::Empty,
            Cell::Num(n) => Val::Num(n),
            Cell::Text(s) => Val::text(s),
            Cell::Bool(b) => Val::Bool(b),
            Cell::Err(e) => Val::Err(e),
        }
    }

    /// 1 つの値として見る（配列なら左上）。
    pub fn cell(&self) -> Cell<'_> {
        match self {
            Val::Empty => Cell::Empty,
            Val::Num(n) => Cell::Num(*n),
            Val::Text(s) => Cell::Text(s),
            Val::Bool(b) => Cell::Bool(*b),
            Val::Err(e) => Cell::Err(*e),
            Val::Array(a) => a.data.first().map_or(Cell::Empty, |v| v.cell()),
        }
    }
}

/// 配列（行優先）。
#[derive(Clone, Debug, PartialEq)]
pub struct Array {
    pub rows: usize,
    pub cols: usize,
    pub data: Vec<Val>,
}

impl Array {
    pub fn new(rows: usize, cols: usize, data: Vec<Val>) -> Array {
        debug_assert_eq!(rows * cols, data.len());
        Array { rows, cols, data }
    }

    pub fn get(&self, r: usize, c: usize) -> &Val {
        &self.data[r * self.cols + c]
    }
}

/// セルの値（借りた形。列をまとめて読むとき）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Cell<'a> {
    Empty,
    Num(f64),
    Text(&'a str),
    Bool(bool),
    Err(Error),
}

/// セルの読み方。行・列は 0 始まりの格子の位置。
pub trait Grid {
    /// シートの名前 → 番号（大文字・小文字を区別しない）。
    fn sheet(&self, name: &str) -> Option<usize>;

    /// セルの値（式なら計算の結果）。
    fn get(&self, sheet: usize, row: u64, col: u32) -> Val;

    /// 使っている範囲（行数・列数）。列全体・行全体の参照はここまでに縮める。
    fn used(&self, sheet: usize) -> (u64, u32);

    /// 1 列の `rows` の値を行の順に渡す（既定は 1 つずつ [`Grid::get`]）。
    fn scan(&self, sheet: usize, col: u32, rows: Range<u64>, f: &mut dyn FnMut(u64, Cell<'_>)) {
        for r in rows {
            let v = self.get(sheet, r, col);
            f(r, v.cell());
        }
    }

    /// 1 列の `rows` の完全一致の索引（列が変わるまで覚えておける側が作る。既定は作らない）。
    fn exact_index(&self, _sheet: usize, _col: u32, _rows: Range<u64>) -> Option<Arc<ExactIndex>> {
        None
    }

    /// 範囲の値が、1 回の再計算の間に変わらないか（式・式の結果を含まない）。`true` なら同じ範囲の
    /// `SUMIFS` をまとめて計算する。
    fn stable(&self, _sheet: usize, _area: &Area) -> bool {
        false
    }
}

/// 範囲の相対参照の行を `dr` 行ずらす（列全体の参照は動かない）。行が負になれば `None`。
pub fn offset_area(a: &Area, dr: i64) -> Option<Area> {
    if dr == 0 || a.kind == AreaKind::Cols {
        return Some(*a);
    }
    let mv = |r: u64, abs: bool| -> Option<u64> {
        if abs {
            Some(r)
        } else {
            u64::try_from(r as i64 + dr).ok()
        }
    };
    let mut out = *a;
    out.r0 = mv(a.r0, a.abs[0])?;
    out.r1 = mv(a.r1, a.abs[2])?;
    if out.r0 > out.r1 {
        std::mem::swap(&mut out.r0, &mut out.r1);
        out.abs.swap(0, 2);
    }
    Some(out)
}

/// 範囲の相対参照を `dr` 行・`dc` 列ずらす（列全体の参照は行が、行全体の参照は列が動かない）。
/// 行・列が範囲の外に出れば `None`。
pub fn offset_area2(a: &Area, dr: i64, dc: i64) -> Option<Area> {
    let mut out = offset_area(a, dr)?;
    if dc == 0 || a.kind == AreaKind::Rows {
        return Some(out);
    }
    let mv = |c: u32, abs: bool| -> Option<u32> {
        if abs {
            Some(c)
        } else {
            u32::try_from(c as i64 + dc).ok().filter(|&c| c <= MAX_COL)
        }
    };
    out.c0 = mv(a.c0, a.abs[1])?;
    out.c1 = mv(a.c1, a.abs[3])?;
    if out.c0 > out.c1 {
        std::mem::swap(&mut out.c0, &mut out.c1);
        out.abs.swap(1, 3);
    }
    Some(out)
}

/// 最後の列（`XFD`）。
pub const MAX_COL: u32 = 16_383;

/// 列番号 → 列の名前（0 → `A`）。
pub fn col_name(mut c: u32) -> String {
    let mut s = Vec::new();
    loop {
        s.push(b'A' + (c % 26) as u8);
        if c < 26 {
            break;
        }
        c = c / 26 - 1;
    }
    s.reverse();
    String::from_utf8(s).expect("ascii")
}

/// 文字列を比べる（大文字・小文字を区別しない）。
pub fn cmp_text(a: &str, b: &str) -> std::cmp::Ordering {
    let x = a.chars().flat_map(char::to_lowercase);
    let y = b.chars().flat_map(char::to_lowercase);
    x.cmp(y)
}

/// 大文字・小文字を区別せずに等しいか。
pub fn eq_text(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.eq_ignore_ascii_case(b) || cmp_text(a, b).is_eq()
}

/// ワイルドカード（`*`・`?`、`~` で打ち消し）で合うか（大文字・小文字を区別しない）。
pub fn wildcard_match(pattern: &str, text: &str) -> bool {
    #[derive(Clone, Copy, PartialEq)]
    enum T {
        Any,
        One,
        Ch(char),
    }
    let mut pat = Vec::new();
    let mut it = pattern.chars().flat_map(char::to_lowercase);
    while let Some(c) = it.next() {
        pat.push(match c {
            '*' => T::Any,
            '?' => T::One,
            '~' => match it.next() {
                Some(n) => T::Ch(n),
                None => T::Ch('~'),
            },
            c => T::Ch(c),
        });
    }
    let txt: Vec<char> = text.chars().flat_map(char::to_lowercase).collect();
    // 貪欲法と戻り
    let (mut p, mut t) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while t < txt.len() {
        match pat.get(p) {
            Some(T::Any) => {
                star = Some((p, t));
                p += 1;
            }
            Some(T::One) => {
                p += 1;
                t += 1;
            }
            Some(T::Ch(c)) if *c == txt[t] => {
                p += 1;
                t += 1;
            }
            _ => match star {
                Some((sp, st)) => {
                    p = sp + 1;
                    t = st + 1;
                    star = Some((sp, st + 1));
                }
                None => return false,
            },
        }
    }
    pat[p..].iter().all(|x| *x == T::Any)
}
