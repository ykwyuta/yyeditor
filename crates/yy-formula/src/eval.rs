//! 式の評価（15 章 7.2）。
//!
//! 型の変換は Excel に合わせる: 四則演算では数値に見える文字列を数値にし（`="3"+1` は 4）、空は 0、
//! 真偽値は 1・0。比較は 数値 < 文字列 < 真偽値、文字列は大文字・小文字を区別しない。配列（範囲）
//! どうしの演算は要素ごとで、結果はスピルする。

use std::cmp::Ordering;
use std::sync::Arc;

use yy_numfmt::{DateSystem, Parsed};

use crate::parse::{Area, BinOp, Expr};
use crate::{Array, Cell, Error, Grid, Val, cmp_text, func};

/// 1 つの配列（スピル）の大きさの上限（セルの数）。
pub(crate) const MAX_ARRAY: u64 = 5_000_000;

/// 評価の環境。
pub struct Context<'a> {
    pub grid: &'a dyn Grid,
    /// 式のあるシート
    pub sheet: usize,
    pub sys: DateSystem,
    /// 1 回の再計算の間の覚え書き（同じ範囲の `SUMIFS` をまとめる。なければまとめない）
    pub cache: Option<&'a crate::Cache>,
}

/// 引数（参照は値にせずに渡す。`SUMIFS` などは列を直接読む）。
pub(crate) enum Arg {
    V(Val),
    /// シート・範囲（使っている範囲に縮めたもの。空なら `None`）
    R(usize, Option<Area>),
}

/// 式を評価する。結果が配列ならスピルする。
pub fn eval(e: &Expr, cx: &Context<'_>) -> Val {
    value(arg(e, cx), cx)
}

/// 範囲を使っている範囲に縮める（列全体・行全体の参照）。
pub(crate) fn clamp(cx: &Context<'_>, sheet: usize, a: &Area) -> Option<Area> {
    let (rows, cols) = cx.grid.used(sheet);
    let mut a = *a;
    if a.r1 == u64::MAX {
        if rows == 0 || a.r0 >= rows {
            return None;
        }
        a.r1 = rows - 1;
    }
    if a.c1 == u32::MAX {
        if cols == 0 || a.c0 >= cols {
            return None;
        }
        a.c1 = cols - 1;
    }
    Some(a)
}

pub(crate) fn arg(e: &Expr, cx: &Context<'_>) -> Arg {
    match e {
        Expr::Ref(r) => {
            let sheet = match &r.sheet {
                None => cx.sheet,
                Some(name) => match cx.grid.sheet(name) {
                    Some(s) => s,
                    None => return Arg::V(Val::Err(Error::Ref)),
                },
            };
            Arg::R(sheet, clamp(cx, sheet, &r.area))
        }
        Expr::Paren(inner) => arg(inner, cx),
        e => Arg::V(eval_expr(e, cx)),
    }
}

/// 引数を値にする（範囲は 1 セルなら値、そうでなければ配列）。
pub(crate) fn value(a: Arg, cx: &Context<'_>) -> Val {
    match a {
        Arg::V(v) => v,
        Arg::R(_, None) => Val::Empty,
        Arg::R(sheet, Some(area)) => {
            if area.rows() == 1 && area.cols() == 1 {
                return cx.grid.get(sheet, area.r0, area.c0);
            }
            let n = area.rows().saturating_mul(area.cols() as u64);
            if n > MAX_ARRAY {
                return Val::Err(Error::Calc);
            }
            let (rows, cols) = (area.rows() as usize, area.cols() as usize);
            let mut data = vec![Val::Empty; rows * cols];
            for (ci, c) in (area.c0..=area.c1).enumerate() {
                cx.grid.scan(sheet, c, area.r0..area.r1 + 1, &mut |r, v| {
                    data[(r - area.r0) as usize * cols + ci] = Val::of(v);
                });
            }
            Val::Array(Arc::new(Array::new(rows, cols, data)))
        }
    }
}

fn eval_expr(e: &Expr, cx: &Context<'_>) -> Val {
    match e {
        Expr::Num(n) => Val::Num(*n),
        Expr::Text(s) => Val::Text(s.clone()),
        Expr::Bool(b) => Val::Bool(*b),
        Expr::Err(x) => Val::Err(*x),
        Expr::Ref(_) | Expr::Paren(_) => value(arg(e, cx), cx),
        Expr::Array(rows) => {
            let cols = rows[0].len();
            let data = rows
                .iter()
                .flat_map(|r| r.iter().map(|x| eval_expr(x, cx)))
                .collect();
            Val::Array(Arc::new(Array::new(rows.len(), cols, data)))
        }
        Expr::Neg(x) => map(&eval(x, cx), &|v| match num(v, cx.sys) {
            Ok(n) => Val::Num(-n),
            Err(e) => Val::Err(e),
        }),
        Expr::Plus(x) => eval(x, cx),
        Expr::Percent(x) => map(&eval(x, cx), &|v| match num(v, cx.sys) {
            Ok(n) => Val::Num(n / 100.0),
            Err(e) => Val::Err(e),
        }),
        Expr::Bin(op, l, r) => {
            let a = eval(l, cx);
            let b = eval(r, cx);
            zip(&a, &b, &|x, y| binary(*op, x, y, cx.sys))
        }
        Expr::Call(f, args) => func::call(f, args, cx),
        Expr::Missing => Val::Empty,
    }
}

/// 要素ごとに当てる（配列でなければそのまま）。
pub(crate) fn map(v: &Val, f: &dyn Fn(&Val) -> Val) -> Val {
    match v {
        Val::Array(a) => Val::Array(Arc::new(Array::new(
            a.rows,
            a.cols,
            a.data.iter().map(f).collect(),
        ))),
        v => f(v),
    }
}

/// 2 つの値を要素ごとに合わせる（大きさが違えば広げ、はみ出た部分は `#N/A`）。
pub(crate) fn zip(a: &Val, b: &Val, f: &dyn Fn(&Val, &Val) -> Val) -> Val {
    let dims = |v: &Val| match v {
        Val::Array(x) => (x.rows, x.cols),
        _ => (1, 1),
    };
    if !matches!(a, Val::Array(_)) && !matches!(b, Val::Array(_)) {
        return f(a, b);
    }
    let (ar, ac) = dims(a);
    let (br, bc) = dims(b);
    let (rows, cols) = (ar.max(br), ac.max(bc));
    // 1 行・1 列の配列は広げる（Excel と同じ）
    let at = |v: &Val, r: usize, c: usize, vr: usize, vc: usize| -> Option<Val> {
        let r = if vr == 1 { 0 } else { r };
        let c = if vc == 1 { 0 } else { c };
        if r >= vr || c >= vc {
            return None;
        }
        Some(match v {
            Val::Array(x) => x.get(r, c).clone(),
            v => v.clone(),
        })
    };
    let mut data = Vec::with_capacity(rows * cols);
    for r in 0..rows {
        for c in 0..cols {
            data.push(match (at(a, r, c, ar, ac), at(b, r, c, br, bc)) {
                (Some(x), Some(y)) => f(&x, &y),
                _ => Val::Err(Error::NA),
            });
        }
    }
    Val::Array(Arc::new(Array::new(rows, cols, data)))
}

/// 数値にする（四則演算の規則）。
pub(crate) fn num(v: &Val, sys: DateSystem) -> Result<f64, Error> {
    match v.cell() {
        Cell::Empty => Ok(0.0),
        Cell::Num(n) => Ok(n),
        Cell::Bool(b) => Ok(b as u8 as f64),
        Cell::Err(e) => Err(e),
        Cell::Text(s) => match yy_numfmt::parse_input(s, sys) {
            Parsed::Number(n, _) => Ok(n),
            _ => Err(Error::Value),
        },
    }
}

/// 文字列にする（連結の規則。数値は「標準」の表記）。
pub(crate) fn text(v: Cell<'_>) -> Result<String, Error> {
    Ok(match v {
        Cell::Empty => String::new(),
        Cell::Num(n) => yy_numfmt::general(n),
        Cell::Text(s) => s.to_owned(),
        Cell::Bool(b) => (if b { "TRUE" } else { "FALSE" }).into(),
        Cell::Err(e) => return Err(e),
    })
}

/// 計算の結果の数値（無限大・NaN は `#NUM!`）。
fn finite(n: f64) -> Val {
    if n.is_finite() {
        Val::Num(n)
    } else {
        Val::Err(Error::Num)
    }
}

fn binary(op: BinOp, a: &Val, b: &Val, sys: DateSystem) -> Val {
    use BinOp::*;
    match op {
        Add | Sub | Mul | Div | Pow => {
            let x = match num(a, sys) {
                Ok(x) => x,
                Err(e) => return Val::Err(e),
            };
            let y = match num(b, sys) {
                Ok(y) => y,
                Err(e) => return Val::Err(e),
            };
            match op {
                Add => finite(x + y),
                Sub => finite(x - y),
                Mul => finite(x * y),
                Div if y == 0.0 => Val::Err(Error::Div0),
                Div => finite(x / y),
                _ if x == 0.0 && y == 0.0 => Val::Err(Error::Num),
                _ if x == 0.0 && y < 0.0 => Val::Err(Error::Div0),
                _ => finite(x.powf(y)),
            }
        }
        Concat => match (text(a.cell()), text(b.cell())) {
            (Ok(x), Ok(y)) => {
                let s = x + &y;
                if s.chars().count() > 32_767 {
                    Val::Err(Error::Value)
                } else {
                    Val::text(&s)
                }
            }
            (Err(e), _) | (_, Err(e)) => Val::Err(e),
        },
        Eq | Ne | Lt | Le | Gt | Ge => {
            let o = match compare(a.cell(), b.cell()) {
                Ok(o) => o,
                Err(e) => return Val::Err(e),
            };
            Val::Bool(match op {
                Eq => o == Ordering::Equal,
                Ne => o != Ordering::Equal,
                Lt => o == Ordering::Less,
                Le => o != Ordering::Greater,
                Gt => o == Ordering::Greater,
                _ => o != Ordering::Less,
            })
        }
    }
}

/// 比べる（数値 < 文字列 < 真偽値。空は相手の型の 0・""・FALSE）。
pub(crate) fn compare(a: Cell<'_>, b: Cell<'_>) -> Result<Ordering, Error> {
    fn class(c: &Cell<'_>) -> u8 {
        match c {
            Cell::Num(_) => 0,
            Cell::Text(_) => 1,
            Cell::Bool(_) => 2,
            _ => 3,
        }
    }
    let fill = |c: Cell<'static>, other: &Cell<'_>| -> Cell<'static> {
        match (c, other) {
            (Cell::Empty, Cell::Num(_)) => Cell::Num(0.0),
            (Cell::Empty, Cell::Text(_)) => Cell::Text(""),
            (Cell::Empty, Cell::Bool(_)) => Cell::Bool(false),
            (Cell::Empty, Cell::Empty) => Cell::Num(0.0),
            (c, _) => c,
        }
    };
    if let Cell::Err(e) = a {
        return Err(e);
    }
    if let Cell::Err(e) = b {
        return Err(e);
    }
    let (a, b) = match (a, b) {
        (Cell::Empty, b) => (fill(Cell::Empty, &b), b),
        (a, Cell::Empty) => {
            let f = fill(Cell::Empty, &a);
            (a, f)
        }
        x => x,
    };
    Ok(match (a, b) {
        (Cell::Num(x), Cell::Num(y)) => num_cmp(x, y),
        (Cell::Text(x), Cell::Text(y)) => cmp_text(x, y),
        (Cell::Bool(x), Cell::Bool(y)) => x.cmp(&y),
        (a, b) => class(&a).cmp(&class(&b)),
    })
}

/// 数値を比べる（有効数字 15 桁で等しければ等しい。Excel と同じ）。
pub(crate) fn num_cmp(x: f64, y: f64) -> Ordering {
    if x == y {
        return Ordering::Equal;
    }
    let scale = x.abs().max(y.abs());
    if (x - y).abs() <= scale * 1e-15 {
        Ordering::Equal
    } else {
        x.partial_cmp(&y).unwrap_or(Ordering::Equal)
    }
}
