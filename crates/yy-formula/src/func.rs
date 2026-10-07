//! 関数（15 章 7.3）。

use std::cmp::Ordering;
use std::sync::Arc;

use crate::criteria::Criterion;
use crate::eval::{Arg, Context, MAX_ARRAY, arg, compare, eval, map, num, num_cmp, text};
use crate::parse::{Area, Expr, Func};
use crate::{Array, Cell, Error, Val, eq_text, wildcard_match};

pub(crate) fn call(f: &Func, args: &[Expr], cx: &Context<'_>) -> Val {
    match f {
        Func::Abs => abs(args, cx),
        Func::Product => product(args, cx),
        Func::Sumifs => ifs(args, cx, true),
        Func::Countifs => ifs(args, cx, false),
        Func::Xlookup => xlookup(args, cx),
        Func::Concat => concat(args, cx),
        Func::Textjoin => textjoin(args, cx),
        Func::Textsplit => textsplit(args, cx),
        Func::Unknown(_) => Val::Err(Error::Name),
    }
}

fn missing(args: &[Expr], i: usize) -> bool {
    matches!(args.get(i), None | Some(Expr::Missing))
}

/// 省略できる数値の引数。
fn opt_num(args: &[Expr], i: usize, default: f64, cx: &Context<'_>) -> Result<f64, Error> {
    if missing(args, i) {
        return Ok(default);
    }
    num(&eval(&args[i], cx), cx.sys)
}

/// 省略できる真偽値の引数。
fn opt_bool(args: &[Expr], i: usize, default: bool, cx: &Context<'_>) -> Result<bool, Error> {
    Ok(opt_num(args, i, default as u8 as f64, cx)? != 0.0)
}

/// 引数の値を行の順（左から右、上から下）に渡す。
fn each_cell(a: &Arg, cx: &Context<'_>, f: &mut dyn FnMut(Cell<'_>)) {
    match a {
        Arg::V(Val::Array(x)) => x.data.iter().for_each(|v| f(v.cell())),
        Arg::V(v) => f(v.cell()),
        Arg::R(_, None) => {}
        Arg::R(sheet, Some(area)) => {
            if area.cols() == 1 {
                cx.grid
                    .scan(*sheet, area.c0, area.r0..area.r1 + 1, &mut |_, v| f(v));
            } else {
                for r in area.r0..=area.r1 {
                    for c in area.c0..=area.c1 {
                        f(cx.grid.get(*sheet, r, c).cell());
                    }
                }
            }
        }
    }
}

// ---- ABS・PRODUCT ----------------------------------------------------------------------

fn abs(args: &[Expr], cx: &Context<'_>) -> Val {
    if args.len() != 1 {
        return Val::Err(Error::Value);
    }
    map(&eval(&args[0], cx), &|v| match num(v, cx.sys) {
        Ok(n) => Val::Num(n.abs()),
        Err(e) => Val::Err(e),
    })
}

fn product(args: &[Expr], cx: &Context<'_>) -> Val {
    if args.is_empty() {
        return Val::Err(Error::Value);
    }
    let mut prod = 1.0;
    let mut any = false;
    for e in args {
        let a = arg(e, cx);
        match &a {
            // 直接書いた値は数値に変える（文字列の数値・真偽値も数える）
            Arg::V(v) if !matches!(v, Val::Array(_)) => {
                if matches!(v, Val::Empty) {
                    continue;
                }
                match num(v, cx.sys) {
                    Ok(n) => {
                        prod *= n;
                        any = true;
                    }
                    Err(e) => return Val::Err(e),
                }
            }
            // 範囲・配列の中は数値だけ（文字列・真偽値・空は無視、エラーは伝える）
            _ => {
                let mut err = None;
                each_cell(&a, cx, &mut |c| match c {
                    Cell::Num(n) => {
                        prod *= n;
                        any = true;
                    }
                    Cell::Err(e) if err.is_none() => err = Some(e),
                    _ => {}
                });
                if let Some(e) = err {
                    return Val::Err(e);
                }
            }
        }
    }
    if !any {
        return Val::Num(0.0);
    }
    if prod.is_finite() {
        Val::Num(prod)
    } else {
        Val::Err(Error::Num)
    }
}

// ---- SUMIFS・COUNTIFS ------------------------------------------------------------------

/// 範囲の引数（列全体の参照は、関わるシートの使っている行のうち最も多い行数に揃える）。
enum Range {
    Area(usize, Area),
    Values(Arc<Array>),
    Scalar(Val),
}

impl Range {
    fn shape(&self) -> (u64, u64) {
        match self {
            Range::Area(_, a) => (a.rows(), a.cols() as u64),
            Range::Values(a) => (a.rows as u64, a.cols as u64),
            Range::Scalar(_) => (1, 1),
        }
    }

    /// セルを（位置〔行優先〕, 値）で渡す。
    fn each(&self, cx: &Context<'_>, f: &mut dyn FnMut(usize, Cell<'_>)) {
        match self {
            Range::Area(sheet, a) => {
                let cols = a.cols() as usize;
                for (ci, c) in (a.c0..=a.c1).enumerate() {
                    cx.grid.scan(*sheet, c, a.r0..a.r1 + 1, &mut |r, v| {
                        f((r - a.r0) as usize * cols + ci, v)
                    });
                }
            }
            Range::Values(a) => a.data.iter().enumerate().for_each(|(i, v)| f(i, v.cell())),
            Range::Scalar(v) => f(0, v.cell()),
        }
    }

    /// 位置の値。
    fn at(&self, cx: &Context<'_>, i: usize) -> Val {
        match self {
            Range::Area(sheet, a) => {
                let cols = a.cols() as usize;
                cx.grid
                    .get(*sheet, a.r0 + (i / cols) as u64, a.c0 + (i % cols) as u32)
            }
            Range::Values(a) => a.data.get(i).cloned().unwrap_or_default(),
            Range::Scalar(v) => v.clone(),
        }
    }
}

/// 範囲の引数をまとめて解決する（列全体の参照の行数を揃える）。
fn ranges(exprs: &[&Expr], cx: &Context<'_>) -> Result<Vec<Range>, Error> {
    let mut resolved = Vec::new();
    let mut whole_rows = 0u64;
    let mut whole_cols = 0u32;
    for e in exprs {
        let e: &Expr = match e {
            Expr::Paren(x) => x,
            e => e,
        };
        match e {
            Expr::Ref(r) => {
                let sheet = match &r.sheet {
                    None => cx.sheet,
                    Some(n) => cx.grid.sheet(n).ok_or(Error::Ref)?,
                };
                let (rows, cols) = cx.grid.used(sheet);
                if r.area.r1 == u64::MAX {
                    whole_rows = whole_rows.max(rows);
                }
                if r.area.c1 == u32::MAX {
                    whole_cols = whole_cols.max(cols);
                }
                resolved.push(Ok((sheet, r.area)));
            }
            e => resolved.push(Err(eval(e, cx))),
        }
    }
    Ok(resolved
        .into_iter()
        .map(|x| match x {
            Ok((sheet, mut a)) => {
                if a.r1 == u64::MAX {
                    a.r1 = whole_rows.max(a.r0 + 1) - 1;
                }
                if a.c1 == u32::MAX {
                    a.c1 = whole_cols.max(a.c0 + 1) - 1;
                }
                Range::Area(sheet, a)
            }
            Err(Val::Array(a)) => Range::Values(a),
            Err(v) => Range::Scalar(v),
        })
        .collect())
}

fn ifs(args: &[Expr], cx: &Context<'_>, sum: bool) -> Val {
    let first = sum as usize;
    if args.len() < first + 2 || (args.len() - first) % 2 != 0 {
        return Val::Err(Error::Value);
    }
    let mut range_exprs: Vec<&Expr> = Vec::new();
    if sum {
        range_exprs.push(&args[0]);
    }
    let mut crit_vals = Vec::new();
    for pair in args[first..].chunks(2) {
        range_exprs.push(&pair[0]);
        crit_vals.push(eval(&pair[1], cx));
    }
    let rs = match ranges(&range_exprs, cx) {
        Ok(r) => r,
        Err(e) => return Val::Err(e),
    };
    let shape = rs[0].shape();
    if rs.iter().any(|r| r.shape() != shape) {
        return Val::Err(Error::Value);
    }
    let (sum_range, crit_ranges) = if sum {
        (Some(&rs[0]), &rs[1..])
    } else {
        (None, &rs[..])
    };
    // 条件が配列なら要素ごとの結果（スピル）
    let one = |crits: &[Cell<'_>]| -> Val {
        let n = (shape.0 * shape.1) as usize;
        let mut mask = vec![u64::MAX; n.div_ceil(64)];
        for (r, c) in crit_ranges.iter().zip(crits) {
            let crit = Criterion::new(*c, cx.sys);
            r.each(cx, &mut |i, v| {
                if !crit.test(v) {
                    mask[i / 64] &= !(1 << (i % 64));
                }
            });
        }
        let on = |i: usize| mask[i / 64] >> (i % 64) & 1 == 1;
        match sum_range {
            None => Val::Num((0..n).filter(|&i| on(i)).count() as f64),
            Some(s) => {
                let mut total = 0.0;
                let mut err = None;
                s.each(cx, &mut |i, v| {
                    if on(i) {
                        match v {
                            Cell::Num(x) => total += x,
                            Cell::Err(e) if err.is_none() => err = Some(e),
                            _ => {}
                        }
                    }
                });
                match err {
                    Some(e) => Val::Err(e),
                    None => Val::Num(total),
                }
            }
        }
    };
    let dims = crit_vals
        .iter()
        .filter_map(|v| match v {
            Val::Array(a) => Some((a.rows, a.cols)),
            _ => None,
        })
        .fold(None, |acc: Option<(usize, usize)>, d| {
            Some(acc.map_or(d, |(r, c)| (r.max(d.0), c.max(d.1))))
        });
    match dims {
        None => {
            let crits: Vec<Cell<'_>> = crit_vals.iter().map(Val::cell).collect();
            one(&crits)
        }
        Some((rows, cols)) => {
            let mut data = Vec::with_capacity(rows * cols);
            for r in 0..rows {
                for c in 0..cols {
                    let crits: Vec<Cell<'_>> = crit_vals
                        .iter()
                        .map(|v| match v {
                            Val::Array(a) => {
                                let rr = if a.rows == 1 { 0 } else { r };
                                let cc = if a.cols == 1 { 0 } else { c };
                                if rr < a.rows && cc < a.cols {
                                    a.get(rr, cc).cell()
                                } else {
                                    Cell::Err(Error::NA)
                                }
                            }
                            v => v.cell(),
                        })
                        .collect();
                    data.push(one(&crits));
                }
            }
            Val::Array(Arc::new(Array::new(rows, cols, data)))
        }
    }
}

// ---- XLOOKUP ----------------------------------------------------------------------

fn xlookup(args: &[Expr], cx: &Context<'_>) -> Val {
    if args.len() < 3 || args.len() > 6 {
        return Val::Err(Error::Value);
    }
    let rs = match ranges(&[&args[1], &args[2]], cx) {
        Ok(r) => r,
        Err(e) => return Val::Err(e),
    };
    let (look, ret) = (&rs[0], &rs[1]);
    let (lr, lc) = look.shape();
    let vertical = lc == 1;
    if !vertical && lr != 1 {
        return Val::Err(Error::Value);
    }
    let n = if vertical { lr } else { lc } as usize;
    let (rr, rc) = ret.shape();
    if (vertical && rr as usize != n) || (!vertical && rc as usize != n) {
        return Val::Err(Error::Value);
    }
    let match_mode = match opt_num(args, 4, 0.0, cx) {
        Ok(m) if [0.0, -1.0, 1.0, 2.0].contains(&m) => m as i32,
        Ok(_) => return Val::Err(Error::Value),
        Err(e) => return Val::Err(e),
    };
    let search_mode = match opt_num(args, 5, 1.0, cx) {
        Ok(m) if [1.0, -1.0, 2.0, -2.0].contains(&m) => m as i32,
        Ok(_) => return Val::Err(Error::Value),
        Err(e) => return Val::Err(e),
    };
    let result = |i: usize| -> Val {
        // 見つけた位置の、戻り範囲の行（縦）・列（横）
        let count = if vertical { rc } else { rr } as usize;
        let cells: Vec<Val> = (0..count)
            .map(|k| {
                let idx = if vertical {
                    i * rc as usize + k
                } else {
                    k * rc as usize + i
                };
                ret.at(cx, idx)
            })
            .collect();
        if cells.len() == 1 {
            cells.into_iter().next().unwrap_or_default()
        } else if vertical {
            Val::Array(Arc::new(Array::new(1, count, cells)))
        } else {
            Val::Array(Arc::new(Array::new(count, 1, cells)))
        }
    };
    let not_found = || {
        if missing(args, 3) {
            Val::Err(Error::NA)
        } else {
            eval(&args[3], cx)
        }
    };
    let find = |key: &Val| -> Val {
        if let Val::Err(e) = key {
            return Val::Err(*e);
        }
        let key = key.cell();
        let found = if search_mode.abs() == 2 {
            binary_search(look, cx, n, key, match_mode, search_mode < 0)
        } else {
            linear_search(look, cx, key, match_mode, search_mode < 0)
        };
        match found {
            Some(i) => result(i),
            None => not_found(),
        }
    };
    match eval(&args[0], cx) {
        Val::Array(keys) => {
            // 検索値が配列なら要素ごと（各結果の左上）
            let data = keys
                .data
                .iter()
                .map(|k| match find(k) {
                    Val::Array(a) => a.data.first().cloned().unwrap_or_default(),
                    v => v,
                })
                .collect();
            Val::Array(Arc::new(Array::new(keys.rows, keys.cols, data)))
        }
        k => find(&k),
    }
}

/// 完全一致か（ワイルドカードのときは文字列をパターンで比べる）。
fn exact(key: Cell<'_>, v: Cell<'_>, wildcard: bool) -> bool {
    match (key, v) {
        (Cell::Text(k), Cell::Text(s)) if wildcard => wildcard_match(k, s),
        (Cell::Text(k), Cell::Text(s)) => eq_text(k, s),
        (Cell::Num(k), Cell::Num(x)) => num_cmp(k, x) == Ordering::Equal,
        (Cell::Bool(k), Cell::Bool(x)) => k == x,
        (Cell::Empty, Cell::Empty) => true,
        _ => false,
    }
}

/// 同じ種類の値どうしで比べる（種類が違えば `None`）。
fn same_kind_cmp(v: Cell<'_>, key: Cell<'_>) -> Option<Ordering> {
    match (v, key) {
        (Cell::Num(_), Cell::Num(_))
        | (Cell::Text(_), Cell::Text(_))
        | (Cell::Bool(_), Cell::Bool(_)) => compare(v, key).ok(),
        _ => None,
    }
}

fn linear_search(
    look: &Range,
    cx: &Context<'_>,
    key: Cell<'_>,
    mode: i32,
    last: bool,
) -> Option<usize> {
    let wildcard = mode == 2;
    let mut hit: Option<usize> = None;
    // 近似一致の候補（位置・値）
    let mut best: Option<(usize, Val)> = None;
    look.each(cx, &mut |i, v| {
        if hit.is_some() && !last {
            return;
        }
        if exact(key, v, wildcard) {
            hit = Some(i);
            return;
        }
        if mode == -1 || mode == 1 {
            let Some(o) = same_kind_cmp(v, key) else {
                return;
            };
            let want = if mode == -1 {
                Ordering::Less
            } else {
                Ordering::Greater
            };
            if o != want {
                return;
            }
            let better = match &best {
                None => true,
                Some((_, b)) => match compare(v, b.cell()) {
                    Ok(Ordering::Equal) => last,
                    Ok(x) => (x == Ordering::Greater) == (mode == -1),
                    Err(_) => false,
                },
            };
            if better {
                best = Some((i, Val::of(v)));
            }
        }
    });
    hit.or(best.map(|b| b.0))
}

fn binary_search(
    look: &Range,
    cx: &Context<'_>,
    n: usize,
    key: Cell<'_>,
    mode: i32,
    descending: bool,
) -> Option<usize> {
    // 「key 以下の最後の位置」を探す（降順なら逆向き）
    let (mut lo, mut hi) = (0usize, n);
    while lo < hi {
        let mid = (lo + hi) / 2;
        let v = look.at(cx, mid);
        let o = compare(v.cell(), key).unwrap_or(Ordering::Greater);
        let before = if descending {
            o != Ordering::Less
        } else {
            o != Ordering::Greater
        };
        if before {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    // lo は key を超える最初の位置。lo - 1 が key 以下の最後
    let at = |i: usize| look.at(cx, i);
    if lo > 0 && exact(key, at(lo - 1).cell(), false) {
        return Some(lo - 1);
    }
    match (mode, descending) {
        (-1, false) | (1, true) => lo.checked_sub(1),
        (1, false) | (-1, true) => (lo < n).then_some(lo),
        _ => None,
    }
}

// ---- 文字列 ------------------------------------------------------------------------

const MAX_TEXT: usize = 32_767;

fn concat(args: &[Expr], cx: &Context<'_>) -> Val {
    let mut out = String::new();
    for e in args {
        let a = arg(e, cx);
        let mut err = None;
        each_cell(&a, cx, &mut |c| match text(c) {
            Ok(s) => out.push_str(&s),
            Err(e) if err.is_none() => err = Some(e),
            Err(_) => {}
        });
        if let Some(e) = err {
            return Val::Err(e);
        }
        if out.len() > MAX_TEXT * 4 {
            return Val::Err(Error::Value);
        }
    }
    if out.chars().count() > MAX_TEXT {
        return Val::Err(Error::Value);
    }
    Val::text(&out)
}

fn textjoin(args: &[Expr], cx: &Context<'_>) -> Val {
    if args.len() < 3 {
        return Val::Err(Error::Value);
    }
    // 区切りは配列でもよい（順に使う）
    let mut delims = Vec::new();
    let mut err = None;
    each_cell(&arg(&args[0], cx), cx, &mut |c| match text(c) {
        Ok(s) => delims.push(s),
        Err(e) => err = Some(e),
    });
    if let Some(e) = err {
        return Val::Err(e);
    }
    if delims.is_empty() {
        delims.push(String::new());
    }
    let ignore = match opt_bool(args, 1, true, cx) {
        Ok(b) => b,
        Err(e) => return Val::Err(e),
    };
    let mut parts = Vec::new();
    for e in &args[2..] {
        let a = arg(e, cx);
        let mut err = None;
        each_cell(&a, cx, &mut |c| match text(c) {
            Ok(s) => {
                if !(ignore && s.is_empty()) {
                    parts.push(s);
                }
            }
            Err(e) if err.is_none() => err = Some(e),
            Err(_) => {}
        });
        if let Some(e) = err {
            return Val::Err(e);
        }
    }
    let mut out = String::new();
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            out.push_str(&delims[(i - 1) % delims.len()]);
        }
        out.push_str(p);
        if out.len() > MAX_TEXT * 4 {
            return Val::Err(Error::Value);
        }
    }
    if out.chars().count() > MAX_TEXT {
        return Val::Err(Error::Value);
    }
    Val::text(&out)
}

/// 区切りの引数（文字列か、文字列の配列）。
fn delimiters(a: &Arg, cx: &Context<'_>) -> Result<Vec<String>, Error> {
    let mut out = Vec::new();
    let mut err = None;
    each_cell(a, cx, &mut |c| match text(c) {
        Ok(s) => {
            if !s.is_empty() {
                out.push(s)
            }
        }
        Err(e) => err = Some(e),
    });
    match err {
        Some(e) => Err(e),
        None => Ok(out),
    }
}

/// 区切りで分ける（区切りが複数なら、どれでも分ける。`fold` なら大文字・小文字を区別しない）。
fn split_by(s: &str, delims: &[String], fold: bool) -> Vec<String> {
    if delims.is_empty() {
        return vec![s.to_owned()];
    }
    let hay = if fold { s.to_lowercase() } else { s.to_owned() };
    let ds: Vec<String> = delims
        .iter()
        .map(|d| if fold { d.to_lowercase() } else { d.clone() })
        .collect();
    // 小文字にして長さが変わる文字があれば区別したまま分ける
    let (hay, ds) = if hay.len() == s.len() {
        (hay, ds)
    } else {
        (s.to_owned(), delims.to_vec())
    };
    let mut out = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < hay.len() {
        if let Some(d) = ds.iter().find(|d| hay[i..].starts_with(d.as_str())) {
            out.push(s[start..i].to_owned());
            i += d.len();
            start = i;
        } else {
            i += hay[i..].chars().next().map_or(1, char::len_utf8);
        }
    }
    out.push(s[start..].to_owned());
    out
}

fn textsplit(args: &[Expr], cx: &Context<'_>) -> Val {
    if args.len() < 2 || args.len() > 6 {
        return Val::Err(Error::Value);
    }
    let src = match eval(&args[0], cx) {
        Val::Array(_) => return Val::Err(Error::Value),
        v => match text(v.cell()) {
            Ok(s) => s,
            Err(e) => return Val::Err(e),
        },
    };
    let col_d = if missing(args, 1) {
        Vec::new()
    } else {
        match delimiters(&arg(&args[1], cx), cx) {
            Ok(d) => d,
            Err(e) => return Val::Err(e),
        }
    };
    let row_d = if missing(args, 2) {
        Vec::new()
    } else {
        match delimiters(&arg(&args[2], cx), cx) {
            Ok(d) => d,
            Err(e) => return Val::Err(e),
        }
    };
    if col_d.is_empty() && row_d.is_empty() {
        return Val::Err(Error::Value);
    }
    let ignore = match opt_bool(args, 3, false, cx) {
        Ok(b) => b,
        Err(e) => return Val::Err(e),
    };
    let fold = match opt_num(args, 4, 0.0, cx) {
        Ok(m) => m == 1.0,
        Err(e) => return Val::Err(e),
    };
    let pad = if missing(args, 5) {
        Val::Err(Error::NA)
    } else {
        eval(&args[5], cx)
    };
    let rows: Vec<Vec<String>> = split_by(&src, &row_d, fold)
        .into_iter()
        .filter(|r| !(ignore && r.is_empty()))
        .map(|r| {
            split_by(&r, &col_d, fold)
                .into_iter()
                .filter(|c| !(ignore && c.is_empty()))
                .collect::<Vec<_>>()
        })
        .filter(|r| !(ignore && r.is_empty()))
        .collect();
    if rows.is_empty() {
        return Val::Err(Error::Calc);
    }
    let cols = rows.iter().map(Vec::len).max().unwrap_or(1).max(1);
    if (rows.len() * cols) as u64 > MAX_ARRAY {
        return Val::Err(Error::Calc);
    }
    let mut data = Vec::with_capacity(rows.len() * cols);
    for r in &rows {
        for c in 0..cols {
            data.push(match r.get(c) {
                Some(s) => Val::text(s),
                None => pad.clone(),
            });
        }
    }
    if data.len() == 1 {
        return data.pop().unwrap_or_default();
    }
    Val::Array(Arc::new(Array::new(rows.len(), cols, data)))
}
