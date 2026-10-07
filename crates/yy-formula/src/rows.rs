//! 共有式（15 章 7.4）: 列の多数の行に入れた「行ごとに相対的に同じ式」を、まとめて計算する。
//!
//! 同じ行のセルの参照と演算だけの式（`=C2*D2`・`=A2&"-"&B2`・`=ABS(E2-F2)` など）は、参照する列を
//! 塊（4096 行）ごとにまとめて読み、行ごとの演算だけを行う（ベクトル化）。それ以外の式（`SUMIFS`・
//! `XLOOKUP` などを含む）は、行ごとに相対参照をずらして評価する。

use std::sync::Arc;

use crate::eval::{Context, binary, eval, num, text};
use crate::parse::{AreaKind, BinOp, Expr, Func};
use crate::{Area, Error, Val, offset_area};

/// まとめて読む行の数。
const BLOCK: u64 = 4096;

/// 式の相対参照の行を `dr` 行ずらした式（行が負になる参照は `#REF!`）。
pub fn shift(e: &Expr, dr: i64) -> Expr {
    match e {
        Expr::Ref(r) => match offset_area(&r.area, dr) {
            Some(area) => Expr::Ref(crate::Ref {
                sheet: r.sheet.clone(),
                area,
            }),
            None => Expr::Err(Error::Ref),
        },
        Expr::Neg(x) => Expr::Neg(Box::new(shift(x, dr))),
        Expr::Plus(x) => Expr::Plus(Box::new(shift(x, dr))),
        Expr::Percent(x) => Expr::Percent(Box::new(shift(x, dr))),
        Expr::Paren(x) => Expr::Paren(Box::new(shift(x, dr))),
        Expr::Bin(op, l, r) => Expr::Bin(*op, Box::new(shift(l, dr)), Box::new(shift(r, dr))),
        Expr::Call(f, args) => Expr::Call(f.clone(), args.iter().map(|a| shift(a, dr)).collect()),
        e => e.clone(),
    }
}

/// 式を `rows` 行に入れたとき、参照する範囲の全体（相対参照の行を広げたもの）。
pub fn spread(a: &Area, rows: u64) -> Area {
    if rows <= 1 || a.kind == AreaKind::Cols {
        return *a;
    }
    let mut out = *a;
    if !a.abs[2] {
        out.r1 = a.r1.saturating_add(rows - 1);
    }
    if !a.abs[0] && a.abs[2] {
        // `A1:$A$10` のように始めだけ動く範囲は、動いた先も含める
        out.r1 = out.r1.max(a.r0.saturating_add(rows - 1));
    }
    out
}

/// まとめて計算できる式。
enum Node {
    Const(Val),
    /// 同じ行（からずれた行）のセル（`slots` の番号）
    Slot(usize),
    Neg(Box<Node>),
    Percent(Box<Node>),
    Bin(BinOp, Box<Node>, Box<Node>),
    Abs(Box<Node>),
    Concat(Vec<Node>),
    /// 完全一致の `XLOOKUP`（検索範囲の索引で引き、戻り範囲の 1 列から読む）
    Lookup {
        key: Box<Node>,
        index: Arc<crate::ExactIndex>,
        /// 戻り範囲（シート・1 列の範囲）
        ret: (usize, Area),
        not_found: Option<Box<Node>>,
        last: bool,
    },
}

/// 行ごとに動かない範囲（絶対参照か列全体）を、使っている範囲に縮めて解決する。
fn fixed_area(e: &Expr, cx: &Context<'_>) -> Option<(usize, Area)> {
    let Expr::Ref(r) = e else {
        return None;
    };
    let a = r.area;
    if a.kind != AreaKind::Cols && !(a.abs[0] && a.abs[2]) {
        return None;
    }
    let sheet = match &r.sheet {
        None => cx.sheet,
        Some(n) => cx.grid.sheet(n)?,
    };
    Some((sheet, crate::eval::clamp(cx, sheet, &a)?))
}

/// 読む列（シート・列・式の 1 行目での行）。
type SlotRef = (usize, u32, u64);

fn compile(e: &Expr, cx: &Context<'_>, slots: &mut Vec<SlotRef>) -> Option<Node> {
    Some(match e {
        Expr::Num(n) => Node::Const(Val::Num(*n)),
        Expr::Text(s) => Node::Const(Val::Text(s.clone())),
        Expr::Bool(b) => Node::Const(Val::Bool(*b)),
        Expr::Err(x) => Node::Const(Val::Err(*x)),
        Expr::Paren(x) | Expr::Plus(x) => compile(x, cx, slots)?,
        Expr::Neg(x) => Node::Neg(Box::new(compile(x, cx, slots)?)),
        Expr::Percent(x) => Node::Percent(Box::new(compile(x, cx, slots)?)),
        Expr::Bin(op, l, r) => Node::Bin(
            *op,
            Box::new(compile(l, cx, slots)?),
            Box::new(compile(r, cx, slots)?),
        ),
        Expr::Ref(r) if r.area.kind == AreaKind::Cell => {
            if r.area.abs[0] {
                // 行が絶対の参照はどの行でも同じ値
                Node::Const(eval(e, cx))
            } else {
                let sheet = match &r.sheet {
                    None => cx.sheet,
                    Some(n) => cx.grid.sheet(n)?,
                };
                let key = (sheet, r.area.c0, r.area.r0);
                let i = slots.iter().position(|s| *s == key).unwrap_or_else(|| {
                    slots.push(key);
                    slots.len() - 1
                });
                Node::Slot(i)
            }
        }
        Expr::Call(Func::Abs, args) if args.len() == 1 => {
            Node::Abs(Box::new(compile(&args[0], cx, slots)?))
        }
        Expr::Call(Func::Xlookup, args) if (3..=6).contains(&args.len()) => {
            let mode = |i: usize, ok: &[f64]| match args.get(i) {
                None | Some(Expr::Missing) => true,
                Some(Expr::Num(n)) => ok.contains(n),
                Some(Expr::Neg(x)) => matches!(**x, Expr::Num(n) if ok.contains(&-n)),
                _ => false,
            };
            if !mode(4, &[0.0]) || !mode(5, &[1.0, -1.0]) {
                return None;
            }
            let last = matches!(args.get(5), Some(Expr::Neg(_)));
            let (ls, la) = fixed_area(&args[1], cx)?;
            let (rs, ra) = fixed_area(&args[2], cx)?;
            if la.cols() != 1 || ra.cols() != 1 || la.rows() != ra.rows() {
                return None;
            }
            let index = cx.grid.exact_index(ls, la.c0, la.r0..la.r1 + 1)?;
            let not_found = match args.get(3) {
                None | Some(Expr::Missing) => None,
                Some(e) => Some(Box::new(compile(e, cx, slots)?)),
            };
            Node::Lookup {
                key: Box::new(compile(&args[0], cx, slots)?),
                index,
                ret: (rs, ra),
                not_found,
                last,
            }
        }
        Expr::Call(Func::Concat, args) => Node::Concat(
            args.iter()
                .map(|a| compile(a, cx, slots))
                .collect::<Option<_>>()?,
        ),
        _ => return None,
    })
}

fn run(n: &Node, vals: &[Val], cx: &Context<'_>) -> Val {
    match n {
        Node::Const(v) => v.clone(),
        Node::Slot(i) => vals[*i].clone(),
        Node::Neg(x) => match num(&run(x, vals, cx), cx.sys) {
            Ok(v) => Val::Num(-v),
            Err(e) => Val::Err(e),
        },
        Node::Percent(x) => match num(&run(x, vals, cx), cx.sys) {
            Ok(v) => Val::Num(v / 100.0),
            Err(e) => Val::Err(e),
        },
        Node::Bin(op, l, r) => binary(*op, &run(l, vals, cx), &run(r, vals, cx), cx.sys),
        Node::Abs(x) => match num(&run(x, vals, cx), cx.sys) {
            Ok(v) => Val::Num(v.abs()),
            Err(e) => Val::Err(e),
        },
        Node::Lookup {
            key,
            index,
            ret: (sheet, area),
            not_found,
            last,
        } => {
            let k = run(key, vals, cx);
            if let Val::Err(e) = k {
                return Val::Err(e);
            }
            match index.find(k.cell(), *last) {
                Some(i) => cx.grid.get(*sheet, area.r0 + i as u64, area.c0),
                None => match not_found {
                    Some(n) => run(n, vals, cx),
                    None => Val::Err(Error::NA),
                },
            }
        }
        Node::Concat(args) => {
            let mut out = String::new();
            for a in args {
                match text(run(a, vals, cx).cell()) {
                    Ok(s) => out.push_str(&s),
                    Err(e) => return Val::Err(e),
                }
            }
            if out.chars().count() > 32_767 {
                Val::Err(Error::Value)
            } else {
                Val::Text(Arc::from(out.as_str()))
            }
        }
    }
}

/// 共有式を `count` 行分計算する（`cx.offset` が 1 行目のずれ）。結果は行の順に `out` に渡す
/// （配列の結果は左上の値）。
pub fn eval_rows(e: &Expr, cx: &Context<'_>, count: u64, out: &mut dyn FnMut(u64, Val)) {
    let top = |v: Val| match v {
        Val::Array(a) => a.data.first().cloned().unwrap_or_default(),
        v => v,
    };
    let mut slots = Vec::new();
    let Some(prog) = compile(e, cx, &mut slots) else {
        for i in 0..count {
            let c = Context {
                offset: cx.offset + i as i64,
                ..*cx
            };
            out(i, top(eval(e, &c)));
        }
        return;
    };
    let mut start = 0;
    while start < count {
        let len = BLOCK.min(count - start);
        // 列を塊ごとに読む
        let cols: Vec<Vec<Val>> = slots
            .iter()
            .map(|&(sheet, col, row)| {
                let first = row as i64 + cx.offset + start as i64;
                let mut v = vec![Val::Empty; len as usize];
                if first < 0 {
                    return vec![Val::Err(Error::Ref); len as usize];
                }
                let first = first as u64;
                cx.grid.scan(sheet, col, first..first + len, &mut |r, c| {
                    v[(r - first) as usize] = Val::of(c);
                });
                v
            })
            .collect();
        let mut vals = vec![Val::Empty; slots.len()];
        for k in 0..len as usize {
            for (s, c) in vals.iter_mut().zip(&cols) {
                *s = c[k].clone();
            }
            out(start + k as u64, run(&prog, &vals, cx));
        }
        start += len;
    }
}
