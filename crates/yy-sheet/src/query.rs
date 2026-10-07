//! 絞り込みと並べ替え（15 章 9）。
//!
//! どちらも表のデータを動かさず、結果を「行のビット列」（絞り込み）・「行番号の並び」（並べ替え）で
//! 返す。チャンクごとに全コアで並列に計算し、絞り込みではチャンクの統計で当たり得ないチャンクを
//! 読み飛ばす。
//!
//! 値の順は Excel と同じ: 数値 < 文字列 < 真偽値 < エラー、空のセルは昇順・降順とも最後。文字列は
//! 大文字・小文字を区別せずに比べる（[`cmp_text`]）。

use std::cmp::Ordering;
use std::io;
use std::sync::Arc;

use rayon::prelude::*;

use crate::Context;
use crate::chunk::{Bitmap, CellRef, Data, FxMap};
use crate::column::Column;
use crate::sheet::Table;
use crate::value::Value;

// ---- 文字列の比べ方 --------------------------------------------------------------------

/// 文字列を比べる（大文字・小文字を区別しない。同じなら元の文字の順）。
pub fn cmp_text(a: &str, b: &str) -> Ordering {
    let mut x = a.chars().flat_map(char::to_lowercase);
    let mut y = b.chars().flat_map(char::to_lowercase);
    loop {
        match (x.next(), y.next()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(p), Some(q)) if p != q => return p.cmp(&q),
            _ => {}
        }
    }
}

/// 大文字・小文字を区別せずに等しいか。
pub fn eq_text(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.eq_ignore_ascii_case(b)
        || a.chars()
            .flat_map(char::to_lowercase)
            .eq(b.chars().flat_map(char::to_lowercase))
}

fn lower(s: &str) -> String {
    s.to_lowercase()
}

/// ワイルドカード（`*`・`?`、`~` で打ち消し）に合うか（大文字・小文字を区別しない）。
pub fn wildcard_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = lower(pattern).chars().collect();
    let t: Vec<char> = lower(text).chars().collect();
    // トークン（None = *、Some(None) = ?、Some(Some(c)) = 文字）
    let mut toks: Vec<Option<Option<char>>> = Vec::new();
    let mut i = 0;
    while i < p.len() {
        match p[i] {
            '~' if i + 1 < p.len() => {
                toks.push(Some(Some(p[i + 1])));
                i += 2;
            }
            '*' => {
                toks.push(None);
                i += 1;
            }
            '?' => {
                toks.push(Some(None));
                i += 1;
            }
            c => {
                toks.push(Some(Some(c)));
                i += 1;
            }
        }
    }
    // 貪欲法と後戻り（* の位置を覚える）
    let (mut ti, mut pi) = (0usize, 0usize);
    let (mut star, mut mark) = (None, 0usize);
    while ti < t.len() {
        match toks.get(pi) {
            Some(Some(None)) => {
                ti += 1;
                pi += 1;
            }
            Some(Some(Some(c))) if *c == t[ti] => {
                ti += 1;
                pi += 1;
            }
            Some(None) => {
                star = Some(pi);
                mark = ti;
                pi += 1;
            }
            _ => match star {
                Some(s) => {
                    pi = s + 1;
                    mark += 1;
                    ti = mark;
                }
                None => return false,
            },
        }
    }
    while toks.get(pi) == Some(&None) {
        pi += 1;
    }
    pi == toks.len()
}

// ---- 条件 ------------------------------------------------------------------------------

/// 比較。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cmp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl Cmp {
    fn test(self, o: Ordering) -> bool {
        match self {
            Cmp::Eq => o == Ordering::Equal,
            Cmp::Ne => o != Ordering::Equal,
            Cmp::Lt => o == Ordering::Less,
            Cmp::Le => o != Ordering::Greater,
            Cmp::Gt => o == Ordering::Greater,
            Cmp::Ge => o != Ordering::Less,
        }
    }
}

/// 文字列の条件。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextOp {
    Equals,
    Contains,
    BeginsWith,
    EndsWith,
    /// ワイルドカード（`*`・`?`）
    Wildcard,
}

/// 値の一覧で選ぶときの値（数値はビット、文字列は小文字）。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    Number(u64),
    Text(String),
    Bool(bool),
    Error(u8),
}

impl Key {
    pub fn of(v: CellRef<'_>) -> Option<Key> {
        Some(match v {
            CellRef::Empty => return None,
            // -0 と 0 は同じ
            CellRef::Number(n) => Key::Number(if n == 0.0 { 0 } else { n.to_bits() }),
            CellRef::Text(s) => Key::Text(lower(s)),
            CellRef::Bool(b) => Key::Bool(b),
            CellRef::Error(e) => Key::Error(e.code()),
        })
    }
}

/// 列の条件。
#[derive(Clone, Debug)]
pub enum Cond {
    /// 値の一覧（チェックボックス）。`blanks` なら空のセルも
    Values {
        values: std::collections::HashSet<Key>,
        blanks: bool,
    },
    /// 文字列（大文字・小文字を区別しない。`negate` なら合わないもの）
    Text {
        op: TextOp,
        pattern: String,
        negate: bool,
    },
    /// 数値の比較
    Number {
        op: Cmp,
        value: f64,
    },
    /// 数値の範囲（含む）
    Between(f64, f64),
    /// 上位・下位（`n` 件、`percent` なら n %）
    Top {
        n: u64,
        bottom: bool,
        percent: bool,
    },
    /// 平均より上・下
    Average {
        above: bool,
    },
    Blank,
    NonBlank,
    And(Box<Cond>, Box<Cond>),
    Or(Box<Cond>, Box<Cond>),
}

/// 列の条件（表の列）。
#[derive(Clone, Debug)]
pub struct ColFilter {
    pub col: u32,
    pub cond: Cond,
}

/// 列全体から決める値（上位の境目・平均）を入れた条件。
#[derive(Clone, Debug)]
enum Ready {
    Values(std::collections::HashSet<Key>, bool),
    Text(TextOp, String, bool),
    Number(Cmp, f64),
    Between(f64, f64),
    Blank,
    NonBlank,
    Never,
    And(Box<Ready>, Box<Ready>),
    Or(Box<Ready>, Box<Ready>),
}

impl Ready {
    fn test(&self, v: CellRef<'_>) -> bool {
        match self {
            Ready::Values(set, blanks) => match Key::of(v) {
                None => *blanks,
                Some(k) => set.contains(&k),
            },
            Ready::Text(op, pat, negate) => {
                let hit = match v {
                    CellRef::Text(s) => text_test(*op, pat, s),
                    CellRef::Number(n) => text_test(*op, pat, &yy_numfmt::general(n)),
                    CellRef::Bool(b) => text_test(*op, pat, if b { "TRUE" } else { "FALSE" }),
                    CellRef::Error(e) => text_test(*op, pat, e.text()),
                    CellRef::Empty => false,
                };
                hit != *negate
            }
            Ready::Number(op, x) => match v {
                CellRef::Number(n) => op.test(n.partial_cmp(x).unwrap_or(Ordering::Equal)),
                _ => *op == Cmp::Ne && !matches!(v, CellRef::Empty),
            },
            Ready::Between(lo, hi) => matches!(v, CellRef::Number(n) if n >= *lo && n <= *hi),
            Ready::Blank => matches!(v, CellRef::Empty),
            Ready::NonBlank => !matches!(v, CellRef::Empty),
            Ready::Never => false,
            Ready::And(a, b) => a.test(v) && b.test(v),
            Ready::Or(a, b) => a.test(v) || b.test(v),
        }
    }

    /// チャンクの統計から、どの行にも合わないことが分かるか。
    fn skip(&self, s: &crate::chunk::Stats, full: bool) -> bool {
        let _ = full;
        use crate::chunk::Stats as S;
        match self {
            Ready::Number(op, x) => {
                // 数値以外の値がなければ、最小・最大で決まる
                let only_numbers = s.kinds & !S::NUMBER == 0;
                if s.nonempty == 0 {
                    // 空のセルは数値の条件に合わない
                    return true;
                }
                if !only_numbers {
                    return false;
                }
                match op {
                    Cmp::Gt => s.max <= *x,
                    Cmp::Ge => s.max < *x,
                    Cmp::Lt => s.min >= *x,
                    Cmp::Le => s.min > *x,
                    Cmp::Eq => *x < s.min || *x > s.max,
                    Cmp::Ne => false,
                }
            }
            Ready::Between(lo, hi) => s.kinds & S::NUMBER == 0 || s.max < *lo || s.min > *hi,
            Ready::NonBlank => s.nonempty == 0,
            Ready::Text(_, _, false) => s.nonempty == 0,
            Ready::Never => true,
            Ready::And(a, b) => a.skip(s, full) || b.skip(s, full),
            Ready::Or(a, b) => a.skip(s, full) && b.skip(s, full),
            _ => false,
        }
    }
}

fn text_test(op: TextOp, pat_lower: &str, s: &str) -> bool {
    match op {
        TextOp::Equals => eq_text(s, pat_lower),
        TextOp::Wildcard => wildcard_match(pat_lower, s),
        _ => {
            let l = lower(s);
            match op {
                TextOp::Contains => l.contains(pat_lower),
                TextOp::BeginsWith => l.starts_with(pat_lower),
                _ => l.ends_with(pat_lower),
            }
        }
    }
}

/// 列のすべての値を順にたどる（チャンクごとに並列に。`f(チャンクの中身, 始まり, 行数)` の結果を集める）。
fn par_pieces<T: Send>(
    ctx: &Context,
    col: &Column,
    f: impl Fn(u64, &Data, usize, usize, &crate::chunk::Stats) -> T + Sync,
) -> io::Result<Vec<T>> {
    let mut starts = Vec::with_capacity(col.pieces().len());
    let mut row = 0u64;
    for p in col.pieces() {
        starts.push(row);
        row += p.len as u64;
    }
    col.pieces()
        .par_iter()
        .zip(starts.par_iter())
        .map(|(p, &start)| {
            let d = p.chunk.data(ctx)?;
            Ok(f(
                start,
                &d,
                p.start as usize,
                p.len as usize,
                &p.chunk.stats,
            ))
        })
        .collect()
}

/// 列の数値を集める（差分を含む）。
fn numbers(ctx: &Context, col: &Column) -> io::Result<Vec<f64>> {
    let parts = par_pieces(ctx, col, |start, d, off, len, _| {
        let mut v = Vec::new();
        for k in 0..len {
            if col.delta().contains_key(&(start + k as u64)) {
                continue;
            }
            if let CellRef::Number(n) = d.get(off + k) {
                v.push(n);
            }
        }
        v
    })?;
    let mut all: Vec<f64> = parts.into_iter().flatten().collect();
    for v in col.delta().values() {
        if let Value::Number(n) = v {
            all.push(*n);
        }
    }
    Ok(all)
}

fn prepare(ctx: &Context, col: &Column, c: &Cond) -> io::Result<Ready> {
    Ok(match c {
        Cond::Values { values, blanks } => Ready::Values(values.clone(), *blanks),
        Cond::Text {
            op,
            pattern,
            negate,
        } => Ready::Text(*op, lower(pattern), *negate),
        Cond::Number { op, value } => Ready::Number(*op, *value),
        Cond::Between(a, b) => Ready::Between(a.min(*b), a.max(*b)),
        Cond::Blank => Ready::Blank,
        Cond::NonBlank => Ready::NonBlank,
        Cond::Top { n, bottom, percent } => {
            let mut nums = numbers(ctx, col)?;
            let k = if *percent {
                (nums.len() as u64 * n).div_ceil(100)
            } else {
                *n
            } as usize;
            if k == 0 || nums.is_empty() {
                Ready::Never
            } else if k >= nums.len() {
                Ready::Between(f64::NEG_INFINITY, f64::INFINITY)
            } else {
                let cmp = |a: &f64, b: &f64| a.partial_cmp(b).unwrap_or(Ordering::Equal);
                if *bottom {
                    let (_, x, _) = nums.select_nth_unstable_by(k - 1, cmp);
                    Ready::Number(Cmp::Le, *x)
                } else {
                    let i = nums.len() - k;
                    let (_, x, _) = nums.select_nth_unstable_by(i, cmp);
                    Ready::Number(Cmp::Ge, *x)
                }
            }
        }
        Cond::Average { above } => {
            let nums = numbers(ctx, col)?;
            if nums.is_empty() {
                Ready::Never
            } else {
                let avg = nums.iter().sum::<f64>() / nums.len() as f64;
                Ready::Number(if *above { Cmp::Gt } else { Cmp::Lt }, avg)
            }
        }
        Cond::And(a, b) => Ready::And(
            Box::new(prepare(ctx, col, a)?),
            Box::new(prepare(ctx, col, b)?),
        ),
        Cond::Or(a, b) => Ready::Or(
            Box::new(prepare(ctx, col, a)?),
            Box::new(prepare(ctx, col, b)?),
        ),
    })
}

/// 1 列の条件に合う行のビット列。
pub fn filter_column(ctx: &Context, table: &Table, f: &ColFilter) -> io::Result<Bitmap> {
    let rows = table.rows as usize;
    let Some(col) = table.columns.get(f.col as usize) else {
        return Ok(Bitmap::new(rows, false));
    };
    let ready = prepare(ctx, col, &f.cond)?;
    let parts = par_pieces(ctx, col, |start, d, off, len, stats| {
        let full = off == 0 && len == d.len();
        let mut b = Bitmap::new(len, false);
        if !ready.skip(stats, full) {
            for k in 0..len {
                if ready.test(d.get(off + k)) {
                    b.set(k, true);
                }
            }
        }
        (start, b)
    })?;
    let mut out = Bitmap::new(rows, false);
    for (start, b) in parts {
        out.copy_from(start as usize, &b);
    }
    for (&r, v) in col.delta() {
        if (r as usize) < rows {
            out.set(r as usize, ready.test(CellRef::of(v)));
        }
    }
    Ok(out)
}

/// すべての列の条件を満たす行のビット列（段階ごとの残りの行数も返す）。
pub fn filter(
    ctx: &Context,
    table: &Table,
    filters: &[ColFilter],
) -> io::Result<(Bitmap, Vec<u64>)> {
    let mut acc = Bitmap::new(table.rows as usize, true);
    let mut counts = Vec::with_capacity(filters.len());
    for f in filters {
        let b = filter_column(ctx, table, f)?;
        acc.and_assign(&b);
        counts.push(acc.count_ones() as u64);
    }
    Ok((acc, counts))
}

// ---- 値の一覧 ------------------------------------------------------------------------

/// 値と件数。
#[derive(Clone, Debug, PartialEq)]
pub struct ValueCount {
    pub value: Value,
    pub count: u64,
}

/// 列の値の一覧（件数付き、値の順）。`mask` があればその行だけ。値の種類が `limit` を超えたら打ち切る
/// （戻り値の 3 つ目が `true`）。2 つ目は空のセルの数。
pub fn value_counts(
    ctx: &Context,
    col: &Column,
    mask: Option<&Bitmap>,
    limit: usize,
) -> io::Result<(Vec<ValueCount>, u64, bool)> {
    type Part = (FxMap<Key, (Value, u64)>, u64, bool);
    let parts: Vec<Part> = par_pieces(ctx, col, |start, d, off, len, _| {
        let mut m: FxMap<Key, (Value, u64)> = FxMap::default();
        let mut blanks = 0u64;
        let mut cut = false;
        for k in 0..len {
            let r = start + k as u64;
            if mask.is_some_and(|b| !b.get(r as usize)) || col.delta().contains_key(&r) {
                continue;
            }
            let v = d.get(off + k);
            match Key::of(v) {
                None => blanks += 1,
                Some(key) => {
                    if let Some(e) = m.get_mut(&key) {
                        e.1 += 1;
                    } else if m.len() < limit {
                        m.insert(key, (v.to_value(), 1));
                    } else {
                        cut = true;
                    }
                }
            }
        }
        (m, blanks, cut)
    })?;
    let mut all: FxMap<Key, (Value, u64)> = FxMap::default();
    let mut blanks = 0;
    let mut cut = false;
    for (m, b, c) in parts {
        blanks += b;
        cut |= c;
        for (k, (v, n)) in m {
            if let Some(e) = all.get_mut(&k) {
                e.1 += n;
            } else if all.len() < limit {
                all.insert(k, (v, n));
            } else {
                cut = true;
            }
        }
    }
    for (&r, v) in col.delta() {
        if mask.is_some_and(|b| !b.get(r as usize)) {
            continue;
        }
        match Key::of(CellRef::of(v)) {
            None => blanks += 1,
            Some(k) => {
                all.entry(k).or_insert_with(|| (v.clone(), 0)).1 += 1;
            }
        }
    }
    let mut out: Vec<ValueCount> = all
        .into_values()
        .map(|(value, count)| ValueCount { value, count })
        .collect();
    out.sort_by(|a, b| cmp_values(&a.value, &b.value));
    Ok((out, blanks, cut))
}

// ---- 並べ替え ------------------------------------------------------------------------

/// 値を比べる（昇順。Excel の順）。
pub fn cmp_values(a: &Value, b: &Value) -> Ordering {
    fn class(v: &Value) -> u8 {
        match v {
            Value::Number(_) => 0,
            Value::Text(_) => 1,
            Value::Bool(_) => 2,
            Value::Error(_) => 3,
            Value::Empty => 4,
        }
    }
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.partial_cmp(y).unwrap_or(Ordering::Equal),
        (Value::Text(x), Value::Text(y)) => cmp_text(x, y),
        (Value::Bool(x), Value::Bool(y)) => x.cmp(y),
        (Value::Error(x), Value::Error(y)) => x.cmp(y),
        _ => class(a).cmp(&class(b)),
    }
}

/// 並べ替えのキー。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SortKey {
    pub col: u32,
    pub desc: bool,
}

/// 順序を保つ 64 ビットの数（f64 のビット）。
fn order_bits(n: f64) -> u64 {
    let n = if n == 0.0 { 0.0 } else { n };
    let b = n.to_bits();
    if b >> 63 == 1 { !b } else { b | (1 << 63) }
}

/// 値の種類（昇順の位置。空は常に最後）。
const C_NUM: u128 = 0;
const C_TEXT: u128 = 1;
const C_BOOL: u128 = 2;
const C_ERR: u128 = 3;
const C_EMPTY: u128 = 4;

/// 1 列のキー（表の行ごとの 128 ビットの数。昇順に並べればその列の順）。
fn column_keys(ctx: &Context, col: &Column, rows: usize, desc: bool) -> io::Result<Vec<u128>> {
    // 文字列の順位: すべてのチャンクの辞書の文字列を集めて並べる
    let datas: Vec<Arc<Data>> = col
        .pieces()
        .par_iter()
        .map(|p| p.chunk.data(ctx))
        .collect::<io::Result<_>>()?;
    let mut texts: Vec<&str> = Vec::new();
    for d in &datas {
        match &**d {
            Data::Text { dict, .. } => texts.extend(dict.iter()),
            Data::Mixed(v) => texts.extend(v.iter().filter_map(|x| match x {
                Value::Text(s) => Some(&**s),
                _ => None,
            })),
            _ => {}
        }
    }
    let delta_texts: Vec<Arc<str>> = col
        .delta()
        .values()
        .filter_map(|v| match v {
            Value::Text(s) => Some(s.clone()),
            _ => None,
        })
        .collect();
    texts.extend(delta_texts.iter().map(|s| &**s));
    texts.par_sort_unstable_by(|a, b| cmp_text(a, b));
    texts.dedup();
    let rank = |s: &str| -> u64 {
        texts
            .binary_search_by(|x| cmp_text(x, s))
            .unwrap_or_else(|i| i) as u64
    };
    let key = |v: CellRef<'_>, text_rank: Option<u64>| -> u128 {
        let (class, payload) = match v {
            CellRef::Empty => return C_EMPTY << 64,
            CellRef::Number(n) => (C_NUM, order_bits(n)),
            CellRef::Text(s) => (C_TEXT, text_rank.unwrap_or_else(|| rank(s))),
            CellRef::Bool(b) => (C_BOOL, b as u64),
            CellRef::Error(e) => (C_ERR, e.code() as u64),
        };
        if desc {
            ((C_ERR - class) << 64) | (!payload) as u128
        } else {
            (class << 64) | payload as u128
        }
    };
    let mut keys = vec![C_EMPTY << 64; rows];
    // チャンクごとに: 辞書の番号 → 順位
    let mut row = 0usize;
    let parts: Vec<(usize, Vec<u128>)> = col
        .pieces()
        .iter()
        .zip(&datas)
        .map(|(p, d)| {
            let start = row;
            row += p.len as usize;
            (start, p, d)
        })
        .collect::<Vec<_>>()
        .into_par_iter()
        .map(|(start, p, d)| {
            let ranks: Option<Vec<u64>> = match &**d {
                Data::Text { dict, .. } => Some(dict.iter().map(rank).collect()),
                _ => None,
            };
            let mut out = Vec::with_capacity(p.len as usize);
            for k in 0..p.len as usize {
                let i = p.start as usize + k;
                let v = d.get(i);
                let tr = match (&**d, &ranks) {
                    (Data::Text { ids, .. }, Some(r)) if matches!(v, CellRef::Text(_)) => {
                        Some(r[ids[i] as usize])
                    }
                    _ => None,
                };
                out.push(key(v, tr));
            }
            (start, out)
        })
        .collect();
    for (start, v) in parts {
        let end = (start + v.len()).min(rows);
        keys[start..end].copy_from_slice(&v[..end - start]);
    }
    for (&r, v) in col.delta() {
        if (r as usize) < rows {
            keys[r as usize] = key(CellRef::of(v), None);
        }
    }
    Ok(keys)
}

/// 行の並べ替え。`rows` があればその行（絞り込みの結果など）だけを並べる（なければ全行）。安定。
pub fn sort(
    ctx: &Context,
    table: &Table,
    keys: &[SortKey],
    rows: Option<&[u32]>,
) -> io::Result<Vec<u32>> {
    let n = table.rows as usize;
    let mut order: Vec<u32> = match rows {
        Some(r) => r.to_vec(),
        None => (0..n as u32).collect(),
    };
    // 後ろのキーから順に安定に並べる
    for k in keys.iter().rev() {
        let Some(col) = table.columns.get(k.col as usize) else {
            continue;
        };
        let kv = column_keys(ctx, col, n, k.desc)?;
        order.par_sort_by_key(|&r| kv[r as usize]);
    }
    Ok(order)
}

#[cfg(test)]
mod tests;
