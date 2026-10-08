//! 入力された文字列の解釈（Excel の日本語の環境に合わせる）。セルへの入力と CSV の取り込みで使う。
//!
//! 数値（桁区切り・指数・百分率・通貨記号）、日付（`2026/10/7`・`2026-10-07`・`2026年10月7日`）、
//! 日時、時刻（`12:30`・`12:30:15`）、真偽値（`TRUE`・`FALSE`）を値にし、付けるべき表示形式も返す。
//! 先頭の 0 が数値として意味を失う値（`00123`）、`+` で始まる値などは文字列のままにする。

use crate::date::{DateSystem, serial_from_date, time_fraction};

/// 解釈の結果。
#[derive(Clone, Debug, PartialEq)]
pub enum Parsed {
    /// 数値と、付けるべき表示形式（`None` なら標準）
    Number(f64, Option<&'static str>),
    Bool(bool),
    /// 文字列のまま
    Text,
}

/// 文字列を解釈する（前後の空白は無視する）。
pub fn parse_input(s: &str, sys: DateSystem) -> Parsed {
    // 前後が空白でなければ trim（Unicode の空白を調べる）を省く
    let b = s.as_bytes();
    let edge = |c: u8| c.is_ascii_whitespace() || c >= 0x80;
    let t = match (b.first(), b.last()) {
        (Some(&f), Some(&l)) if !edge(f) && !edge(l) => s,
        _ => s.trim(),
    };
    if t.is_empty() {
        return Parsed::Text;
    }
    let b = t.as_bytes();
    // 速い道: 符号・数字・小数点だけの数値（取り込みのほとんど）
    if let Some(v) = plain_number(b) {
        return Parsed::Number(v, None);
    }
    match b[0] {
        b't' | b'T' if t.eq_ignore_ascii_case("TRUE") => return Parsed::Bool(true),
        b'f' | b'F' if t.eq_ignore_ascii_case("FALSE") => return Parsed::Bool(false),
        _ => {}
    }
    let numeric_start =
        matches!(b[0], b'-' | b'.' | b'0'..=b'9') || t.starts_with(['¥', '￥', '$']);
    if numeric_start && let Some(p) = number(t) {
        return p;
    }
    if b[0].is_ascii_digit()
        && b.iter().any(|&c| matches!(c, b'/' | b'-' | b':' | 0xE5))
        && let Some(p) = date_time(t, sys)
    {
        return p;
    }
    Parsed::Text
}

/// 10 の累乗（正確に表せる 10^22 まで）。
const POW10: [f64; 23] = [
    1e0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9, 1e10, 1e11, 1e12, 1e13, 1e14, 1e15, 1e16,
    1e17, 1e18, 1e19, 1e20, 1e21, 1e22,
];

/// 符号・数字・小数点だけの数値（`-12.5`・`0.3`）。先頭の 0 が意味を失う整数（`00123`）は除く。
fn plain_number(b: &[u8]) -> Option<f64> {
    if b.len() > 24 {
        return None;
    }
    let digits = b.strip_prefix(b"-").unwrap_or(b);
    let (int, frac) = match digits.iter().position(|&c| c == b'.') {
        Some(p) => (&digits[..p], Some(&digits[p + 1..])),
        None => (digits, None),
    };
    if !int.iter().all(u8::is_ascii_digit)
        || !frac.is_none_or(|f| f.iter().all(u8::is_ascii_digit))
        || (int.is_empty() && frac.is_none_or(<[u8]>::is_empty))
        || (int.len() > 1 && int[0] == b'0')
    {
        return None;
    }
    // 速い道: 仮数が 2^53 以下で、10 の指数が 22 以下なら、割り算 1 回で正しく丸められる
    let frac = frac.unwrap_or(&[]);
    if int.len() + frac.len() <= 15 && frac.len() <= 22 {
        let mut m: u64 = 0;
        for &c in int.iter().chain(frac) {
            m = m * 10 + (c - b'0') as u64;
        }
        let v = m as f64 / POW10[frac.len()];
        return Some(if b[0] == b'-' { -v } else { v });
    }
    // ASCII だけなので UTF-8 として正しい
    std::str::from_utf8(b).ok()?.parse::<f64>().ok()
}

/// 数値（`-1,234.5`・`1e5`・`15%`・`¥1,000`・`$-3`・`(100)` は扱わない）。
fn number(t: &str) -> Option<Parsed> {
    let (neg, rest) = match t.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, t),
    };
    let (currency, rest) = match rest
        .strip_prefix('¥')
        .or_else(|| rest.strip_prefix('￥'))
        .map(|r| (Some("¥#,##0;¥-#,##0"), r))
        .or_else(|| {
            rest.strip_prefix('$')
                .map(|r| (Some("$#,##0_);($#,##0)"), r))
        }) {
        Some((c, r)) => (c, r),
        None => (None, rest),
    };
    let (rest, neg) = match (neg, rest.strip_prefix('-')) {
        // ¥-100 の形
        (false, Some(r)) if currency.is_some() => (r, true),
        _ => (rest, neg),
    };
    let (body, percent) = match rest.strip_suffix('%') {
        Some(b) => (b, true),
        None => (rest, false),
    };
    let b = body.as_bytes();
    if b.is_empty() || !(b[0].is_ascii_digit() || b[0] == b'.') {
        return None;
    }
    // 整数部（桁区切りは 3 桁ごとのときだけ）
    let mut i = 0;
    let mut int_digits = String::new();
    let mut grouped = false;
    while i < b.len() && (b[i].is_ascii_digit() || b[i] == b',') {
        if b[i] == b',' {
            grouped = true;
        } else {
            int_digits.push(b[i] as char);
        }
        i += 1;
    }
    let int_part = &body[..i];
    if grouped {
        let groups: Vec<&str> = int_part.split(',').collect();
        if groups[0].is_empty() || groups[0].len() > 3 || groups[1..].iter().any(|g| g.len() != 3) {
            return None;
        }
    }
    let mut frac = String::new();
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            frac.push(b[i] as char);
            i += 1;
        }
        if int_digits.is_empty() && frac.is_empty() {
            return None;
        }
    }
    let mut exp = String::new();
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let start = i;
        i += 1;
        if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
            exp.push(b[i] as char);
            i += 1;
        }
        let digits_start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            exp.push(b[i] as char);
            i += 1;
        }
        if i == digits_start || grouped || percent || currency.is_some() {
            let _ = start;
            return None;
        }
    }
    if i != b.len() {
        return None;
    }
    // 先頭の 0 が意味を失う整数（00123）は文字列のまま
    if int_digits.len() > 1 && int_digits.starts_with('0') && !grouped {
        return None;
    }
    let text = format!(
        "{}{}{}{}",
        if int_digits.is_empty() {
            "0"
        } else {
            &int_digits
        },
        if frac.is_empty() { "" } else { "." },
        frac,
        if exp.is_empty() {
            String::new()
        } else {
            format!("e{exp}")
        }
    );
    let mut v: f64 = text.parse().ok()?;
    if !v.is_finite() {
        return None;
    }
    if percent {
        v /= 100.0;
    }
    if neg {
        v = -v;
    }
    let format = if let Some(c) = currency {
        Some(c)
    } else if percent {
        Some(if frac.is_empty() { "0%" } else { "0.00%" })
    } else if !exp.is_empty() {
        Some("0.00E+00")
    } else if grouped {
        Some(if frac.is_empty() { "#,##0" } else { "#,##0.00" })
    } else {
        None
    };
    Some(Parsed::Number(v, format))
}

/// 数字だけの部分を読む（1 桁以上、`max` 桁まで）。
fn digits(s: &str, max: usize) -> Option<(u32, &str)> {
    let n = s.bytes().take_while(u8::is_ascii_digit).count();
    if n == 0 || n > max {
        return None;
    }
    Some((s[..n].parse().ok()?, &s[n..]))
}

/// 時刻（`h:mm`・`h:mm:ss`・`h:mm:ss.000`）の日の割合。
fn time(s: &str) -> Option<f64> {
    let (h, r) = digits(s, 2)?;
    let r = r.strip_prefix(':')?;
    let (m, r) = digits(r, 2)?;
    let (sec, ms, r) = match r.strip_prefix(':') {
        Some(r) => {
            let (sec, r) = digits(r, 2)?;
            match r.strip_prefix('.') {
                Some(r) => {
                    let n = r.bytes().take_while(u8::is_ascii_digit).count();
                    if n == 0 || n > 3 {
                        return None;
                    }
                    let ms: u32 = format!("{:0<3}", &r[..n]).parse().ok()?;
                    (sec, ms, &r[n..])
                }
                None => (sec, 0, r),
            }
        }
        None => (0, 0, r),
    };
    if !r.is_empty() || h > 23 || m > 59 || sec > 59 {
        return None;
    }
    Some(time_fraction(h, m, sec, ms))
}

fn date_time(t: &str, sys: DateSystem) -> Option<Parsed> {
    // 時刻だけ
    if let Some(f) = time(t) {
        return Some(Parsed::Number(
            f,
            Some(if t.matches(':').count() == 2 {
                "h:mm:ss"
            } else {
                "h:mm"
            }),
        ));
    }
    let (date_part, time_part) = match t.split_once(' ') {
        Some((d, tm)) => (d, Some(tm.trim())),
        None => (t, None),
    };
    let (y, m, d) = ymd(date_part)?;
    let day = serial_from_date(sys, y, m, d)?;
    match time_part {
        None => Some(Parsed::Number(day, Some("yyyy/m/d"))),
        Some(tm) => {
            let f = time(tm)?;
            Some(Parsed::Number(
                day + f,
                Some(if tm.matches(':').count() == 2 {
                    "yyyy/m/d h:mm:ss"
                } else {
                    "yyyy/m/d h:mm"
                }),
            ))
        }
    }
}

/// 年月日（`2026/10/7`・`2026-10-07`・`2026年10月7日`）。
fn ymd(s: &str) -> Option<(i32, u32, u32)> {
    if let Some(r) = s.strip_suffix('日') {
        let (y, r) = digits(r, 4)?;
        let r = r.strip_prefix('年')?;
        let (m, r) = digits(r, 2)?;
        let r = r.strip_prefix('月')?;
        let (d, r) = digits(r, 2)?;
        return r.is_empty().then_some((y as i32, m, d));
    }
    let sep = if s.contains('/') { '/' } else { '-' };
    let mut it = s.split(sep);
    let (y, m, d) = (it.next()?, it.next()?, it.next()?);
    if it.next().is_some() || y.len() != 4 {
        return None;
    }
    let (y, r1) = digits(y, 4)?;
    let (m, r2) = digits(m, 2)?;
    let (d, r3) = digits(d, 2)?;
    (r1.is_empty() && r2.is_empty() && r3.is_empty()).then_some((y as i32, m, d))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Parsed {
        parse_input(s, DateSystem::D1900)
    }

    #[test]
    fn numbers() {
        assert_eq!(p("123"), Parsed::Number(123.0, None));
        assert_eq!(p("-1.5"), Parsed::Number(-1.5, None));
        assert_eq!(p(".5"), Parsed::Number(0.5, None));
        assert_eq!(p("0.25"), Parsed::Number(0.25, None));
        assert_eq!(p("1,234"), Parsed::Number(1234.0, Some("#,##0")));
        assert_eq!(p("1,234.50"), Parsed::Number(1234.5, Some("#,##0.00")));
        assert_eq!(p("15%"), Parsed::Number(0.15, Some("0%")));
        assert_eq!(p("1e3"), Parsed::Number(1000.0, Some("0.00E+00")));
        assert_eq!(p("¥1,000"), Parsed::Number(1000.0, Some("¥#,##0;¥-#,##0")));
        assert_eq!(p("TRUE"), Parsed::Bool(true));
        assert_eq!(p("false"), Parsed::Bool(false));
        for t in [
            "00123", "1,23", "12,3456", "+1", "1-2", "abc", "1.2.3", "1e", "", "--1",
        ] {
            assert_eq!(p(t), Parsed::Text, "{t}");
        }
    }

    #[test]
    fn dates_and_times() {
        assert_eq!(p("2026/10/7"), Parsed::Number(46302.0, Some("yyyy/m/d")));
        assert_eq!(p("2026-10-07"), Parsed::Number(46302.0, Some("yyyy/m/d")));
        assert_eq!(
            p("2026年10月7日"),
            Parsed::Number(46302.0, Some("yyyy/m/d"))
        );
        assert_eq!(
            p("2026/10/7 18:00"),
            Parsed::Number(46302.75, Some("yyyy/m/d h:mm"))
        );
        assert_eq!(p("12:00"), Parsed::Number(0.5, Some("h:mm")));
        assert_eq!(p("2026/2/30"), Parsed::Text);
        assert_eq!(p("25:00"), Parsed::Text);
        assert_eq!(p("26/10/7"), Parsed::Text);
    }
}
