//! オートフィル（フィルハンドルのドラッグ・ダブルクリック。15 章 12.4）。
//!
//! Excel と同じく、元のセルの並びから「連続データ」にするか「コピー」にするかを決める:
//!
//! - 数値 1 つはコピー（Ctrl で連続データ）、日付 1 つは 1 日ずつ。数値・日付が 2 つ以上なら直線の傾向
//!   （最小二乗）で続ける。
//! - 末尾（なければ先頭）に数字のある文字列（`項目1`・`No.001`・`1月` 以外）は数字を増やす。
//! - 曜日・月などの一覧（`月`・`月曜日`・`1月`・`Jan`・`第1四半期`・干支など）は一覧を順に続ける。
//! - それ以外・種類の混ざった並びは、元の並びを繰り返す（Ctrl で連続データとコピーを入れ替える）。
//! - 式は相対参照をずらしてコピーする。1 つの式を下へ広げるときは共有式（[`crate::Sheet::fill_down`]）。

use std::io;

use crate::Context;
use crate::sheet::Sheet;
use crate::style::Rect;
use crate::value::Value;
use yy_numfmt::DateSystem;

/// 一度に書くセルの数の上限（式を下へ広げる共有式は数えない）。
pub const MAX_CELLS: u64 = 2_000_000;

/// 書式をセルごとに写すときの上限（超えれば列・行ごとに 1 つ目の書式を使う）。
const STYLE_CELLS: u64 = 10_000;

/// 元のセル。
#[derive(Clone, Debug, PartialEq)]
pub struct Src {
    pub value: Value,
    /// 日付・時刻の表示形式
    pub date: bool,
}

impl Src {
    pub fn new(value: Value) -> Src {
        Src { value, date: false }
    }
}

/// 続ける一覧（大文字・小文字は区別しない）。
const LISTS: &[&[&str]] = &[
    &["日", "月", "火", "水", "木", "金", "土"],
    &[
        "日曜日",
        "月曜日",
        "火曜日",
        "水曜日",
        "木曜日",
        "金曜日",
        "土曜日",
    ],
    &[
        "1月", "2月", "3月", "4月", "5月", "6月", "7月", "8月", "9月", "10月", "11月", "12月",
    ],
    &["第1四半期", "第2四半期", "第3四半期", "第4四半期"],
    &[
        "子", "丑", "寅", "卯", "辰", "巳", "午", "未", "申", "酉", "戌", "亥",
    ],
    &["甲", "乙", "丙", "丁", "戊", "己", "庚", "辛", "壬", "癸"],
    &["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"],
    &[
        "Sunday",
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
    ],
    &[
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ],
    &[
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ],
];

/// 英字の大文字・小文字の書き方。
#[derive(Clone, Copy, Debug, PartialEq)]
enum Case {
    AsList,
    Upper,
    Lower,
}

#[derive(Clone, Debug, PartialEq)]
enum Kind {
    Num(f64),
    List(usize, usize, Case),
    TextNum {
        pre: String,
        num: u64,
        /// 0 で埋める桁数（`001` なら 3。埋めないなら 0）
        width: usize,
        post: String,
    },
    Other,
}

fn kind(v: &Value) -> Kind {
    match v {
        Value::Number(x) => Kind::Num(*x),
        Value::Text(t) => {
            let t: &str = t;
            for (li, list) in LISTS.iter().enumerate() {
                if let Some(pos) = list.iter().position(|x| x.eq_ignore_ascii_case(t)) {
                    let case = if !t.chars().any(|c| c.is_ascii_alphabetic()) || list[pos] == t {
                        Case::AsList
                    } else if t.chars().all(|c| !c.is_ascii_lowercase()) {
                        Case::Upper
                    } else if t.chars().all(|c| !c.is_ascii_uppercase()) {
                        Case::Lower
                    } else {
                        Case::AsList
                    };
                    return Kind::List(li, pos, case);
                }
            }
            text_num(t).unwrap_or(Kind::Other)
        }
        _ => Kind::Other,
    }
}

/// 末尾（なければ先頭）の数字。
fn text_num(t: &str) -> Option<Kind> {
    let b = t.as_bytes();
    let tail = b.iter().rev().take_while(|c| c.is_ascii_digit()).count();
    let (pre, digits, post) = if tail > 0 {
        (&t[..t.len() - tail], &t[t.len() - tail..], "")
    } else {
        let head = b.iter().take_while(|c| c.is_ascii_digit()).count();
        if head == 0 {
            return None;
        }
        ("", &t[..head], &t[head..])
    };
    if digits.len() > 15 {
        return None;
    }
    Some(Kind::TextNum {
        pre: pre.into(),
        num: digits.parse().ok()?,
        width: if digits.len() > 1 && digits.starts_with('0') {
            digits.len()
        } else {
            0
        },
        post: post.into(),
    })
}

/// 有効数字 15 桁に丸める（`0.1` ずつ足した誤差を消す。Excel と同じ）。
fn round15(x: f64) -> f64 {
    if x == 0.0 || !x.is_finite() {
        return x;
    }
    format!("{x:.14e}").parse().unwrap_or(x)
}

/// 直線の当てはめ（x は 0, 1, …）。傾きと切片。
fn fit(ys: &[f64]) -> (f64, f64) {
    let n = ys.len() as f64;
    let mx = (n - 1.0) / 2.0;
    let my = ys.iter().sum::<f64>() / n;
    let (mut sxy, mut sxx) = (0.0, 0.0);
    for (i, y) in ys.iter().enumerate() {
        let dx = i as f64 - mx;
        sxy += dx * (y - my);
        sxx += dx * dx;
    }
    let m = if sxx == 0.0 { 0.0 } else { sxy / sxx };
    (m, my - m * mx)
}

/// フィルの仕方（Excel の「オートフィル オプション」）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FillMode {
    /// 元の並びから決める（Ctrl で入れ替え）
    Auto,
    /// セルのコピー（並びを繰り返す）
    Copy,
    /// 連続データ（数値 1 つも 1 ずつ増やす）
    Series,
    /// 日付を日単位・週日単位（土日を飛ばす）・月単位・年単位で
    Days,
    Weekdays,
    Months,
    Years,
}

/// フィルの指定。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FillOptions {
    pub mode: FillMode,
    /// Ctrl を押しながら離した（`Auto` で連続データとコピーを入れ替える）
    pub ctrl: bool,
    /// 値（式）を入れる（`false` なら書式のみコピー）
    pub values: bool,
    /// 書式を写す（`false` なら書式なしコピー）
    pub formats: bool,
    /// 日付のシリアル値の基準
    pub sys: DateSystem,
}

impl FillOptions {
    pub fn auto(ctrl: bool, sys: DateSystem) -> FillOptions {
        FillOptions {
            mode: FillMode::Auto,
            ctrl,
            values: true,
            formats: true,
            sys,
        }
    }
}

/// 値の続け方（線ごと）。
#[derive(Clone, Copy, Debug)]
pub struct How {
    pub mode: FillMode,
    pub ctrl: bool,
    /// 上・左へ続ける（元は呼ぶ側で逆に並べる。1 つだけのときは減らす向き）
    pub backward: bool,
    pub sys: DateSystem,
}

impl How {
    fn auto(ctrl: bool) -> How {
        How {
            mode: FillMode::Auto,
            ctrl,
            backward: false,
            sys: DateSystem::D1900,
        }
    }
}

/// 連続データにできる並びか（種類がそろっているか）。
fn seriesable(kinds: &[Kind]) -> bool {
    match kinds {
        [] => false,
        [k] => !matches!(k, Kind::Other),
        ks => {
            let all_num = ks.iter().all(|k| matches!(k, Kind::Num(_)));
            let same_list = match &ks[0] {
                Kind::List(l, ..) => ks.iter().all(|k| matches!(k, Kind::List(m, ..) if m == l)),
                _ => false,
            };
            let same_text = match &ks[0] {
                Kind::TextNum { pre, post, .. } => ks.iter().all(
                    |k| matches!(k, Kind::TextNum { pre: p, post: q, .. } if p == pre && q == post),
                ),
                _ => false,
            };
            all_num || same_list || same_text
        }
    }
}

/// 連続データにするか（`false` ならコピー）。`ctrl` は Ctrl を押しながら離した。
pub fn is_series(src: &[Src], ctrl: bool) -> bool {
    series_with(src, &How::auto(ctrl))
}

fn series_with(src: &[Src], how: &How) -> bool {
    let kinds: Vec<Kind> = src.iter().map(|s| kind(&s.value)).collect();
    if !seriesable(&kinds) {
        return false;
    }
    match how.mode {
        FillMode::Copy => false,
        FillMode::Series => true,
        FillMode::Days | FillMode::Weekdays | FillMode::Months | FillMode::Years => true,
        FillMode::Auto => {
            // 数値 1 つはコピー、日付 1 つは連続データ（Ctrl で入れ替え）
            let base = match kinds.as_slice() {
                [Kind::Num(_)] => src[0].date,
                _ => true,
            };
            base != how.ctrl
        }
    }
}

/// `src` の続きの `n` 個の値（`src` の並びの向きに、近い順。Excel のオートフィルと同じ決め方）。
pub fn extend(src: &[Src], n: usize, ctrl: bool) -> Vec<Value> {
    extend_with(src, n, &How::auto(ctrl))
}

/// 年・月・日（と 1 日の中の時刻）。
fn ymd(sys: DateSystem, x: f64) -> Option<(i32, u32, u32, f64)> {
    let day = x.floor();
    let dt = yy_numfmt::date::datetime_from_serial(sys, day)?;
    Some((dt.year, dt.month, dt.day, x - day))
}

fn days_in_month(y: i32, m: u32) -> u32 {
    (28..=31)
        .rev()
        .find(|&d| yy_numfmt::date::valid_date(y, m, d))
        .unwrap_or(28)
}

/// 月を足した日付（日は元の日、月末を超えれば月末。Excel の `EDATE`）。
fn add_months(sys: DateSystem, x: f64, months: i64) -> Option<f64> {
    let (y, m, d, frac) = ymd(sys, x)?;
    let total = y as i64 * 12 + (m as i64 - 1) + months;
    let (ny, nm) = (total.div_euclid(12) as i32, total.rem_euclid(12) as u32 + 1);
    let nd = d.min(days_in_month(ny, nm));
    Some(yy_numfmt::date::serial_from_date(sys, ny, nm, nd)? + frac)
}

/// 月の番号（年 × 12 ＋ 月）。
fn month_index(sys: DateSystem, x: f64) -> Option<i64> {
    let (y, m, ..) = ymd(sys, x)?;
    Some(y as i64 * 12 + m as i64)
}

/// 曜日（0 が日曜）。
fn weekday(sys: DateSystem, x: f64) -> Option<i64> {
    let (y, m, d, _) = ymd(sys, x)?;
    Some((yy_numfmt::date::days_from_civil(y, m, d) + 4).rem_euclid(7))
}

/// 日付が月ずつ（同じ日、または月末で丸めた日）に並んでいれば、その月の数。
fn month_step(sys: DateSystem, xs: &[f64]) -> Option<i64> {
    let step = month_index(sys, xs[1])? - month_index(sys, xs[0])?;
    if step == 0 {
        return None;
    }
    for (j, &x) in xs.iter().enumerate() {
        if add_months(sys, xs[0], step * j as i64)?.floor() != x.floor() {
            return None;
        }
    }
    Some(step)
}

/// 数値（日付）の続き。
fn extend_numbers(src: &[Src], ys: &[f64], n: usize, how: &How) -> Vec<Value> {
    let k = ys.len();
    let sign = if how.backward { -1.0 } else { 1.0 };
    let sys = how.sys;
    let num = |x: Option<f64>| {
        x.map(Value::Number)
            .unwrap_or(Value::Error(crate::CellError::Num))
    };
    // 月単位・年単位（指定、または日付が月ずつ並んでいるとき）
    let months = match how.mode {
        FillMode::Months | FillMode::Years => {
            let unit = if how.mode == FillMode::Years { 12 } else { 1 };
            Some(if k == 1 {
                unit * sign as i64
            } else {
                match (month_index(sys, ys[0]), month_index(sys, ys[1])) {
                    (Some(a), Some(b)) if b != a => {
                        // 年単位は年の差に丸める
                        if unit == 12 {
                            ((b - a) as f64 / 12.0).round().max(1.0) as i64 * 12 * (b - a).signum()
                        } else {
                            b - a
                        }
                    }
                    _ => unit * sign as i64,
                }
            })
        }
        FillMode::Auto if k >= 2 && src.iter().all(|s| s.date) => month_step(sys, ys),
        _ => None,
    };
    if let Some(step) = months {
        return (0..n)
            .map(|i| num(add_months(sys, ys[0], step * (k + i) as i64)))
            .collect();
    }
    if how.mode == FillMode::Weekdays {
        let dir = if k >= 2 && ys[k - 1] < ys[0] || k == 1 && how.backward {
            -1.0
        } else {
            1.0
        };
        let mut x = ys[k - 1];
        return (0..n)
            .map(|_| {
                x += dir;
                // 土日を飛ばす
                while matches!(weekday(sys, x), Some(0 | 6)) {
                    x += dir;
                }
                Value::Number(x)
            })
            .collect();
    }
    let (m, b) = if k == 1 { (sign, ys[0]) } else { fit(ys) };
    (0..n)
        .map(|i| Value::Number(round15(b + m * (k + i) as f64)))
        .collect()
}

/// `src` の続きの `n` 個の値（`how` の仕方で）。
pub fn extend_with(src: &[Src], n: usize, how: &How) -> Vec<Value> {
    let k = src.len();
    if k == 0 {
        return vec![Value::Empty; n];
    }
    if !series_with(src, how) {
        return (0..n).map(|i| src[i % k].value.clone()).collect();
    }
    let sign: i64 = if how.backward { -1 } else { 1 };
    let kinds: Vec<Kind> = src.iter().map(|s| kind(&s.value)).collect();
    match &kinds[0] {
        Kind::Num(_) => {
            let ys: Vec<f64> = kinds
                .iter()
                .map(|k| match k {
                    Kind::Num(x) => *x,
                    _ => 0.0,
                })
                .collect();
            extend_numbers(src, &ys, n, how)
        }
        Kind::List(li, ..) => {
            let list = LISTS[*li];
            let len = list.len() as i64;
            let pos: Vec<i64> = kinds
                .iter()
                .map(|k| match k {
                    Kind::List(_, p, _) => *p as i64,
                    _ => 0,
                })
                .collect();
            let step = if k == 1 { sign } else { pos[1] - pos[0] };
            let case = match kinds[k - 1] {
                Kind::List(_, _, c) => c,
                _ => Case::AsList,
            };
            (0..n)
                .map(|i| {
                    let p = (pos[k - 1] + step * (i as i64 + 1)).rem_euclid(len) as usize;
                    let s = list[p];
                    Value::text(&match case {
                        Case::AsList => s.to_string(),
                        Case::Upper => s.to_ascii_uppercase(),
                        Case::Lower => s.to_ascii_lowercase(),
                    })
                })
                .collect()
        }
        Kind::TextNum {
            pre, width, post, ..
        } => {
            let nums: Vec<f64> = kinds
                .iter()
                .map(|k| match k {
                    Kind::TextNum { num, .. } => *num as f64,
                    _ => 0.0,
                })
                .collect();
            let step = if k == 1 {
                sign
            } else {
                fit(&nums).0.round() as i64
            };
            let last = nums[k - 1] as i64;
            (0..n)
                .map(|i| {
                    // 負になれば絶対値（Excel と同じ）
                    let v = (last + step * (i as i64 + 1)).unsigned_abs();
                    Value::text(&format!("{pre}{v:0width$}{post}", width = *width))
                })
                .collect()
        }
        Kind::Other => (0..n).map(|i| src[i % k].value.clone()).collect(),
    }
}

/// オートフィルの結果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filled {
    /// 連続データ（の列・行がある）
    Series,
    Copy,
    /// 範囲を縮めて消した
    Cleared,
    Nothing,
}

/// 範囲（上・左・下・右。含む）。
pub type Range4 = (u64, u32, u64, u32);

impl Sheet {
    fn is_date_at(&self, row: u64, col: u32) -> bool {
        self.format_at(row, col)
            .is_some_and(|f| yy_numfmt::format::parsed(&f).is_date())
    }

    /// 選択範囲 `src` のフィルハンドルを `dst` まで動かす（`dst` は `src` を 1 方向に広げたか、左上を
    /// そろえて縮めた範囲）。`opts` は仕方（連続データ・コピー・書式のみ・書式なし・日付の単位、Ctrl）。
    /// 絞り込み・並べ替えをしないときに、格子の位置で指定する。
    pub fn autofill(
        &mut self,
        ctx: &Context,
        src: Range4,
        dst: Range4,
        opts: &FillOptions,
    ) -> Result<Filled, String> {
        if self.view.rows.is_some() {
            return Err("絞り込み・並べ替えの表示中はフィルできません".into());
        }
        let (t, l, b, r) = src;
        let (dt, dl, db, dr) = dst;
        if src == dst {
            return Ok(Filled::Nothing);
        }
        let io = |e: io::Error| e.to_string();
        // 縮めたなら外れた部分を消す
        if dt == t && dl == l && db <= b && dr <= r {
            let cells = (b - t + 1) * (r - dr) as u64 + (b - db) * (dr - l + 1) as u64;
            if cells > MAX_CELLS {
                return Err(format!("一度に消せるのは {MAX_CELLS} セルまでです"));
            }
            let (rows, cols) = self.extent();
            for row in t..=b.min(rows.saturating_sub(1)) {
                for col in l..=r.min(cols.saturating_sub(1)) {
                    if row > db || col > dr {
                        self.set(ctx, row, col, Value::Empty).map_err(io)?;
                    }
                }
            }
            if db < b {
                self.styles.clear(Rect::new(db + 1, l, b, r));
            }
            if dr < r {
                self.styles.clear(Rect::new(t, dr + 1, db, r));
            }
            return Ok(Filled::Cleared);
        }
        let vertical = dl == l && dr == r && (dt < t || db > b) && dt <= t && db >= b;
        let horizontal = dt == t && db == b && (dl < l || dr > r) && dl <= l && dr >= r;
        if !(vertical ^ horizontal) {
            return Err("フィルは 1 方向にだけ広げられます".into());
        }
        let forward = if vertical { db > b } else { dr > r };
        let k = if vertical {
            b - t + 1
        } else {
            (r - l + 1) as u64
        };
        let n = if vertical {
            (t - dt) + (db - b)
        } else {
            ((l - dl) + (dr - r)) as u64
        };
        let lines: Vec<u64> = if vertical {
            (l as u64..=r as u64).collect()
        } else {
            (t..=b).collect()
        };
        // 線（列か行）の i 番目のセル（i は元の範囲の始めからの位置。負なら前）
        let at = |line: u64, i: i64| -> (u64, u32) {
            if vertical {
                ((t as i64 + i) as u64, line as u32)
            } else {
                (line, (l as i64 + i) as u32)
            }
        };
        let target = |q: usize| -> i64 {
            if forward {
                k as i64 + q as i64
            } else {
                -(q as i64 + 1)
            }
        };
        // 1 つの式を下へ広げる列は共有式にする（セルの数に数えない）
        let down_formula = |s: &Sheet, line: u64| {
            vertical && forward && k == 1 && s.formula_at(t, line as u32).is_some()
        };
        let written: u64 = if !opts.values {
            0
        } else {
            lines.iter().filter(|&&ln| !down_formula(self, ln)).count() as u64 * n
        };
        if written > MAX_CELLS {
            return Err(format!(
                "一度にフィルできるのは {} セルまでです（1 つの式を下へ広げるのは何行でもできます）",
                MAX_CELLS
            ));
        }
        let mut series = false;
        for &line in &lines {
            let cells: Vec<(u64, u32)> = (0..k as i64).map(|i| at(line, i)).collect();
            // 書式: 線の元の書式がそろっていれば 1 つの範囲で、違えばセルごとに
            let styles: Vec<_> = cells.iter().map(|&(rw, c)| self.style_at(rw, c)).collect();
            let span = if vertical {
                if forward {
                    Rect::new(b + 1, line as u32, db, line as u32)
                } else {
                    Rect::new(dt, line as u32, t - 1, line as u32)
                }
            } else if forward {
                Rect::new(line, r + 1, line, dr)
            } else {
                Rect::new(line, dl, line, l - 1)
            };
            if opts.formats {
                self.styles.clear(span);
                if styles.iter().all(|s| *s == styles[0]) || n * lines.len() as u64 > STYLE_CELLS {
                    self.styles.set(span, styles[0].clone());
                } else {
                    for q in 0..n as usize {
                        let p = target(q);
                        let s = p.rem_euclid(k as i64) as usize;
                        let (rw, c) = at(line, p);
                        self.styles.set(Rect::new(rw, c, rw, c), styles[s].clone());
                    }
                }
            }
            if !opts.values {
                continue;
            }
            if down_formula(self, line) {
                self.fill_down(ctx, t, db, line as u32, line as u32)?;
                continue;
            }
            let formulas: Vec<_> = cells
                .iter()
                .map(|&(rw, c)| self.formula_at(rw, c))
                .collect();
            if formulas.iter().any(Option::is_some) {
                // 式を含む: 元の並びを繰り返し、式は相対参照をずらす
                let values: Vec<Value> = cells
                    .iter()
                    .map(|&(rw, c)| self.get(ctx, rw, c))
                    .collect::<io::Result<_>>()
                    .map_err(io)?;
                for q in 0..n as usize {
                    let p = target(q);
                    let s = p.rem_euclid(k as i64);
                    let (rw, c) = at(line, p);
                    match &formulas[s as usize] {
                        Some(f) => {
                            let d = p - s;
                            let (dr_, dc_) = if vertical { (d, 0) } else { (0, d) };
                            let e = yy_formula::shift_by(&f.expr, dr_, dc_);
                            self.set_formula(ctx, rw, c, &yy_formula::formula_text(&e))?;
                        }
                        None => self
                            .set(ctx, rw, c, values[s as usize].clone())
                            .map_err(io)?,
                    }
                }
                continue;
            }
            let mut srcs: Vec<Src> = cells
                .iter()
                .map(|&(rw, c)| {
                    Ok(Src {
                        value: self.get(ctx, rw, c)?,
                        date: self.is_date_at(rw, c),
                    })
                })
                .collect::<io::Result<_>>()
                .map_err(io)?;
            if !forward {
                srcs.reverse();
            }
            let how = How {
                mode: opts.mode,
                ctrl: opts.ctrl,
                backward: !forward,
                sys: opts.sys,
            };
            series |= series_with(&srcs, &how);
            for (q, v) in extend_with(&srcs, n as usize, &how).into_iter().enumerate() {
                let (rw, c) = at(line, target(q));
                self.set(ctx, rw, c, v).map_err(io)?;
            }
        }
        Ok(if series { Filled::Series } else { Filled::Copy })
    }

    /// 元の範囲に日付（日付の表示形式の数値）があるか（オートフィル オプションに日付の単位を出す）。
    pub fn fill_has_dates(&self, ctx: &Context, src: Range4) -> bool {
        let (t, l, b, r) = src;
        (t..=b)
            .flat_map(|row| (l..=r).map(move |c| (row, c)))
            .take(1000)
            .any(|(row, c)| {
                self.is_date_at(row, c) && matches!(self.get(ctx, row, c), Ok(Value::Number(_)))
            })
    }

    /// フィルハンドルのダブルクリックで、下へ広げる最後の行（Excel と同じく、左の列、なければ右の列の
    /// 続いた値の終わり。表の列なら表の終わり）。広げられなければ `None`。
    pub fn fill_down_end(&self, ctx: &Context, src: Range4) -> Option<u64> {
        let (t, l, b, r) = src;
        let filled = |row: u64, col: u32| {
            self.get(ctx, row, col)
                .map(|v| !v.is_empty())
                .unwrap_or(false)
        };
        let mut sides = Vec::new();
        if l > 0 {
            sides.push(l - 1);
        }
        if r < yy_formula::MAX_COL {
            sides.push(r + 1);
        }
        for col in sides {
            if !filled(b + 1, col) && !filled(t, col) {
                continue;
            }
            let end = if matches!(self.place(b + 1, col), crate::Place::Data(..)) {
                // 表の列は空のセルを探さず、表の終わりまで
                self.table_grid_rows() - 1
            } else {
                let mut row = b;
                let mut budget = 1_000_000u32;
                while filled(row + 1, col) && budget > 0 {
                    row += 1;
                    budget -= 1;
                }
                row
            };
            if end > b {
                return Some(end);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nums(v: &[f64]) -> Vec<Src> {
        v.iter().map(|&x| Src::new(Value::Number(x))).collect()
    }
    fn texts(v: &[&str]) -> Vec<Src> {
        v.iter().map(|&x| Src::new(Value::text(x))).collect()
    }
    fn show(v: Vec<Value>) -> Vec<String> {
        v.iter().map(Value::general_text).collect()
    }

    #[test]
    fn numbers_and_dates() {
        // 数値 1 つはコピー、Ctrl で連続データ
        assert_eq!(show(extend(&nums(&[5.0]), 3, false)), ["5", "5", "5"]);
        assert_eq!(show(extend(&nums(&[5.0]), 3, true)), ["6", "7", "8"]);
        // 日付 1 つは 1 日ずつ、Ctrl でコピー
        let d = [Src {
            value: Value::Number(45000.0),
            date: true,
        }];
        assert_eq!(show(extend(&d, 2, false)), ["45001", "45002"]);
        assert_eq!(show(extend(&d, 2, true)), ["45000", "45000"]);
        // 2 つ以上は傾向
        assert_eq!(show(extend(&nums(&[1.0, 3.0]), 3, false)), ["5", "7", "9"]);
        assert_eq!(show(extend(&nums(&[0.1, 0.2]), 2, false)), ["0.3", "0.4"]);
        // Excel と同じ最小二乗（1, 2, 4 → 5.333…, 6.833…）
        let v = extend(&nums(&[1.0, 2.0, 4.0]), 2, false);
        assert!(matches!(v[0], Value::Number(x) if (x - 16.0 / 3.0).abs() < 1e-9));
        assert!(matches!(v[1], Value::Number(x) if (x - 41.0 / 6.0).abs() < 1e-9));
        // 2 つ以上を Ctrl でコピー（繰り返し）
        assert_eq!(show(extend(&nums(&[1.0, 2.0]), 3, true)), ["1", "2", "1"]);
    }

    #[test]
    fn text_with_numbers_and_lists() {
        assert_eq!(
            show(extend(&texts(&["項目1"]), 2, false)),
            ["項目2", "項目3"]
        );
        assert_eq!(
            show(extend(&texts(&["No.009"]), 2, false)),
            ["No.010", "No.011"]
        );
        assert_eq!(show(extend(&texts(&["1番"]), 1, false)), ["2番"]);
        assert_eq!(show(extend(&texts(&["a1", "a3"]), 2, false)), ["a5", "a7"]);
        assert_eq!(
            show(extend(&texts(&["項目1"]), 2, true)),
            ["項目1", "項目1"]
        );
        assert_eq!(show(extend(&texts(&["金"]), 3, false)), ["土", "日", "月"]);
        assert_eq!(
            show(extend(&texts(&["11月"]), 3, false)),
            ["12月", "1月", "2月"]
        );
        assert_eq!(
            show(extend(&texts(&["月曜日", "水曜日"]), 2, false)),
            ["金曜日", "日曜日"]
        );
        assert_eq!(show(extend(&texts(&["MON"]), 2, false)), ["TUE", "WED"]);
        assert_eq!(show(extend(&texts(&["dec"]), 1, false)), ["jan"]);
        assert_eq!(
            show(extend(&texts(&["第4四半期"]), 1, false)),
            ["第1四半期"]
        );
        // 文字だけ・混ざったものは繰り返す
        assert_eq!(
            show(extend(&texts(&["東京", "大阪"]), 3, false)),
            ["東京", "大阪", "東京"]
        );
        let mixed = vec![Src::new(Value::text("a")), Src::new(Value::Number(1.0))];
        assert_eq!(show(extend(&mixed, 2, false)), ["a", "1"]);
        // 逆向き（呼ぶ側が元を逆にする）: 1, 2 の上は 0, -1
        assert_eq!(show(extend(&nums(&[2.0, 1.0]), 2, false)), ["0", "-1"]);
    }

    fn opts() -> FillOptions {
        FillOptions::auto(false, DateSystem::D1900)
    }

    fn sheet_with(ctx: &Context, cells: &[((u64, u32), &str)]) -> Sheet {
        let mut s = Sheet::new("Sheet1");
        for &((r, c), t) in cells {
            if t.starts_with('=') {
                s.set_formula(ctx, r, c, t).unwrap();
            } else {
                let v = match t.parse::<f64>() {
                    Ok(x) => Value::Number(x),
                    Err(_) => Value::text(t),
                };
                s.set(ctx, r, c, v).unwrap();
            }
        }
        s
    }

    fn text_at(s: &Sheet, ctx: &Context, r: u64, c: u32) -> String {
        match s.formula_at(r, c) {
            Some(f) => f.text.to_string(),
            None => s.get(ctx, r, c).unwrap().general_text(),
        }
    }

    #[test]
    fn sheet_fill_down_up_right_and_shrink() {
        let ctx = Context::for_tests();
        let mut s = sheet_with(&ctx, &[((1, 0), "1"), ((2, 0), "2"), ((1, 1), "=A2*10")]);
        // 下へ: 数値は傾向、式はずらす
        let f = s
            .autofill(&ctx, (1, 0, 2, 0), (1, 0, 5, 0), &opts())
            .unwrap();
        assert_eq!(f, Filled::Series);
        let col: Vec<_> = (1..=5).map(|r| text_at(&s, &ctx, r, 0)).collect();
        assert_eq!(col, ["1", "2", "3", "4", "5"]);
        s.autofill(&ctx, (1, 1, 1, 1), (1, 1, 3, 1), &opts())
            .unwrap();
        assert_eq!(text_at(&s, &ctx, 3, 1), "=A4*10");
        // 上へ
        s.autofill(&ctx, (1, 0, 2, 0), (0, 0, 2, 0), &opts())
            .unwrap();
        assert_eq!(text_at(&s, &ctx, 0, 0), "0");
        // 右へ（式の列をずらす）
        s.autofill(&ctx, (1, 1, 1, 1), (1, 1, 1, 3), &opts())
            .unwrap();
        assert_eq!(text_at(&s, &ctx, 1, 3), "=C2*10");
        // 縮めると消える
        assert_eq!(
            s.autofill(&ctx, (1, 0, 5, 0), (1, 0, 3, 0), &opts())
                .unwrap(),
            Filled::Cleared
        );
        assert_eq!(text_at(&s, &ctx, 4, 0), "");
        assert_eq!(text_at(&s, &ctx, 3, 0), "3");
        // 2 方向は不可
        assert!(
            s.autofill(&ctx, (1, 0, 1, 0), (1, 0, 2, 1), &opts())
                .is_err()
        );
    }

    #[test]
    fn sheet_fill_copies_formula_cycle_and_styles() {
        let ctx = Context::for_tests();
        let mut s = sheet_with(&ctx, &[((0, 0), "x"), ((1, 0), "=B2")]);
        s.styles.set(
            Rect::new(0, 0, 0, 0),
            crate::style::Style {
                bold: Some(true),
                ..Default::default()
            },
        );
        s.autofill(&ctx, (0, 0, 1, 0), (0, 0, 5, 0), &opts())
            .unwrap();
        let col: Vec<_> = (0..=5).map(|r| text_at(&s, &ctx, r, 0)).collect();
        assert_eq!(col, ["x", "=B2", "x", "=B4", "x", "=B6"]);
        assert_eq!(s.style_at(4, 0).bold, Some(true));
        assert_eq!(s.style_at(5, 0).bold, None);
    }

    #[test]
    fn single_formula_down_becomes_shared() {
        let ctx = Context::for_tests();
        let mut s = sheet_with(&ctx, &[((0, 0), "=B1+1")]);
        s.autofill(&ctx, (0, 0, 0, 0), (0, 0, 99_999, 0), &opts())
            .unwrap();
        assert_eq!(text_at(&s, &ctx, 99_999, 0), "=B100000+1");
        assert!(!s.formulas.shared.is_empty());
    }

    fn date(y: i32, m: u32, d: u32) -> f64 {
        yy_numfmt::date::serial_from_date(DateSystem::D1900, y, m, d).unwrap()
    }

    fn dates_of(v: Vec<Value>) -> Vec<(i32, u32, u32)> {
        v.iter()
            .map(|x| match x {
                Value::Number(n) => {
                    let (y, m, d, _) = ymd(DateSystem::D1900, *n).unwrap();
                    (y, m, d)
                }
                _ => panic!("{x:?}"),
            })
            .collect()
    }

    fn how(mode: FillMode, backward: bool) -> How {
        How {
            mode,
            ctrl: false,
            backward,
            sys: DateSystem::D1900,
        }
    }

    #[test]
    fn date_units() {
        let d = |y, m, dd| Src {
            value: Value::Number(date(y, m, dd)),
            date: true,
        };
        // 月末: 1/31 → 2/28 → 3/31（元の日を保つ。EDATE と同じ）
        let v = extend_with(&[d(2026, 1, 31)], 3, &how(FillMode::Months, false));
        assert_eq!(dates_of(v), [(2026, 2, 28), (2026, 3, 31), (2026, 4, 30)]);
        // 月ずつ並んだ日付は自動で月単位（1/31, 2/28 → 3/31）
        let v = extend(&[d(2026, 1, 31), d(2026, 2, 28)], 2, false);
        assert_eq!(dates_of(v), [(2026, 3, 31), (2026, 4, 30)]);
        let v = extend(&[d(2026, 1, 15), d(2026, 3, 15)], 2, false);
        assert_eq!(dates_of(v), [(2026, 5, 15), (2026, 7, 15)]);
        // 日数の差が同じでも、月がそろわなければ日単位
        let v = extend(&[d(2026, 1, 1), d(2026, 1, 8)], 1, false);
        assert_eq!(dates_of(v), [(2026, 1, 15)]);
        // 年単位（うるう日は 2/28 に）
        let v = extend_with(&[d(2024, 2, 29)], 2, &how(FillMode::Years, false));
        assert_eq!(dates_of(v), [(2025, 2, 28), (2026, 2, 28)]);
        // 週日単位（2026/10/9 は金曜 → 月・火）
        let v = extend_with(&[d(2026, 10, 9)], 2, &how(FillMode::Weekdays, false));
        assert_eq!(dates_of(v), [(2026, 10, 12), (2026, 10, 13)]);
        // 上へ（1 つだけなら減らす）
        let v = extend_with(&[d(2026, 3, 31)], 2, &how(FillMode::Months, true));
        assert_eq!(dates_of(v), [(2026, 2, 28), (2026, 1, 31)]);
        let v = extend_with(&[d(2026, 10, 12)], 1, &how(FillMode::Weekdays, true));
        assert_eq!(dates_of(v), [(2026, 10, 9)]);
        let v = extend_with(&[d(2026, 1, 10)], 2, &how(FillMode::Auto, true));
        assert_eq!(dates_of(v), [(2026, 1, 9), (2026, 1, 8)]);
        // 数値の指定: コピー・連続データ
        assert_eq!(
            show(extend_with(
                &nums(&[1.0, 2.0]),
                2,
                &how(FillMode::Copy, false)
            )),
            ["1", "2"]
        );
        assert_eq!(
            show(extend_with(&nums(&[7.0]), 2, &how(FillMode::Series, false))),
            ["8", "9"]
        );
        assert_eq!(
            show(extend_with(&texts(&["水"]), 2, &how(FillMode::Auto, true))),
            ["火", "月"]
        );
    }

    #[test]
    fn formats_only_and_values_only() {
        let ctx = Context::for_tests();
        let mut s = sheet_with(&ctx, &[((0, 0), "1"), ((2, 0), "keep")]);
        s.styles.set(
            Rect::new(0, 0, 0, 0),
            crate::style::Style {
                bold: Some(true),
                ..Default::default()
            },
        );
        let only_formats = FillOptions {
            values: false,
            ..opts()
        };
        s.autofill(&ctx, (0, 0, 0, 0), (0, 0, 2, 0), &only_formats)
            .unwrap();
        assert_eq!(text_at(&s, &ctx, 2, 0), "keep");
        assert_eq!(s.style_at(2, 0).bold, Some(true));
        let mut s = sheet_with(&ctx, &[((0, 0), "1")]);
        s.styles.set(
            Rect::new(0, 0, 0, 0),
            crate::style::Style {
                bold: Some(true),
                ..Default::default()
            },
        );
        let no_formats = FillOptions {
            formats: false,
            mode: FillMode::Series,
            ..opts()
        };
        s.autofill(&ctx, (0, 0, 0, 0), (0, 0, 2, 0), &no_formats)
            .unwrap();
        assert_eq!(text_at(&s, &ctx, 2, 0), "3");
        assert_eq!(s.style_at(2, 0).bold, None);
        // 日付の表示形式があれば日付の単位を出す
        let mut s = sheet_with(&ctx, &[((0, 0), "45000")]);
        assert!(!s.fill_has_dates(&ctx, (0, 0, 0, 0)));
        s.styles.set(
            Rect::new(0, 0, 0, 0),
            crate::style::Style {
                num_fmt: Some("yyyy/m/d".into()),
                ..Default::default()
            },
        );
        assert!(s.fill_has_dates(&ctx, (0, 0, 0, 0)));
    }

    #[test]
    fn double_click_end() {
        let ctx = Context::for_tests();
        let cells: Vec<((u64, u32), String)> = (0..10).map(|r| ((r, 0), "1".to_string())).collect();
        let cells: Vec<((u64, u32), &str)> = cells.iter().map(|(p, t)| (*p, t.as_str())).collect();
        let s = sheet_with(&ctx, &cells);
        assert_eq!(s.fill_down_end(&ctx, (0, 1, 0, 1)), Some(9));
        assert_eq!(s.fill_down_end(&ctx, (0, 3, 0, 3)), None);
    }
}
