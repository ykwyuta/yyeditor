//! 数字編集項目（`PIC ZZ,ZZ9.99-`・`$$$,$$9.99CR`・`***9.99` など）の書き方と読み方。
//!
//! - ゼロ抑制（`Z`・`*`）: 最初の有効な桁（0 でない桁か `9`・小数点）までの桁と挿入文字（`,`・`B`・
//!   `0`・`/`）を空白（`*` なら `*`）にする。値が 0 で `9` がなければ全体を空白（`*` なら小数点以外を
//!   `*`）にする。`BLANK WHEN ZERO` なら 0 は全体を空白。
//! - 浮動挿入（`$$$`・`+++`・`---`）: 最初の記号以外は桁で、抑制した位置の最後に記号を置く。
//! - 固定の符号（`+`・`-`・`CR`・`DB`）と通貨記号。

use crate::{Decimal, Sym};

/// 浮動挿入の記号か。
fn floating_kind(s: Sym) -> Option<u8> {
    match s {
        Sym::Cur(_) => Some(0),
        Sym::Plus => Some(1),
        Sym::Minus => Some(2),
        _ => None,
    }
}

fn is_insert(s: Sym) -> bool {
    matches!(s, Sym::Comma | Sym::B | Sym::Zero | Sym::Slash)
}

/// 浮動挿入の記号の位置（2 つ以上続くとき。間の挿入文字は飛ばす。小数点の前まで）。
fn floating_run(syms: &[Sym]) -> Vec<usize> {
    let Some(first) = syms.iter().position(|s| floating_kind(*s).is_some()) else {
        return Vec::new();
    };
    let k = floating_kind(syms[first]);
    let mut run = vec![first];
    for (i, s) in syms.iter().enumerate().skip(first + 1) {
        if floating_kind(*s) == k {
            run.push(i);
        } else if !is_insert(*s) {
            break;
        }
    }
    if run.len() < 2 { Vec::new() } else { run }
}

/// 数字の位置（記号の番号, 整数部か）。浮動挿入は最初の記号を除いて桁になる。
pub(crate) fn digit_positions(syms: &[Sym]) -> Vec<(usize, bool)> {
    let run = floating_run(syms);
    let point = syms
        .iter()
        .position(|s| matches!(s, Sym::Dot | Sym::V))
        .unwrap_or(syms.len());
    syms.iter()
        .enumerate()
        .filter(|(i, s)| {
            matches!(s, Sym::Nine | Sym::Z | Sym::Star)
                || (run.contains(i) && Some(i) != run.first())
        })
        .map(|(i, _)| (i, i < point))
        .collect()
}

/// 文字の幅（`CR`・`DB` は 2、`V`・`P` は 0）。
fn width(s: Sym) -> usize {
    match s {
        Sym::V | Sym::P => 0,
        Sym::Cr | Sym::Db => 2,
        _ => 1,
    }
}

/// 値（小数部の桁数は項目に合わせてあること）を書く。整数部があふれたら上の桁を落として `true`。
pub(crate) fn format(syms: &[Sym], d: Decimal, blank_zero: bool) -> (String, bool) {
    let pos = digit_positions(syms);
    let total: usize = syms.iter().map(|s| width(*s)).sum();
    let neg = d.value < 0;
    let mut digits = format!("{:0w$}", d.value.unsigned_abs(), w = pos.len());
    let overflow = digits.len() > pos.len();
    if overflow {
        digits = digits[digits.len() - pos.len()..].to_string();
    }
    let zero = digits.bytes().all(|b| b == b'0');
    let has_nine = syms.contains(&Sym::Nine);
    let star = syms.contains(&Sym::Star);
    if zero && (blank_zero || (!has_nine && !star)) {
        return (" ".repeat(total), overflow);
    }
    if zero && !has_nine && star {
        let s = syms
            .iter()
            .map(|s| match s {
                Sym::Dot => ".".to_string(),
                s => "*".repeat(width(*s)),
            })
            .collect();
        return (s, overflow);
    }
    let digit_of: Vec<Option<char>> = {
        let mut v = vec![None; syms.len()];
        for (k, (i, _)) in pos.iter().enumerate() {
            v[*i] = Some(digits.as_bytes()[k] as char);
        }
        v
    };
    // 最初の有効な位置
    let sig = syms
        .iter()
        .enumerate()
        .position(|(i, s)| {
            matches!(s, Sym::Nine | Sym::Dot | Sym::V) || digit_of[i].is_some_and(|c| c != '0')
        })
        .unwrap_or(syms.len());
    let run = floating_run(syms);
    let mut out: Vec<String> = Vec::with_capacity(syms.len());
    for (i, s) in syms.iter().enumerate() {
        let sup = i < sig;
        let piece = match s {
            Sym::V | Sym::P => String::new(),
            Sym::Nine => digit_of[i].unwrap_or('0').to_string(),
            Sym::Z | Sym::Star if sup => (if *s == Sym::Star { "*" } else { " " }).into(),
            _ if run.contains(&i) => {
                if sup {
                    " ".into()
                } else {
                    digit_of[i].unwrap_or('0').to_string()
                }
            }
            Sym::Z | Sym::Star => digit_of[i].unwrap_or('0').to_string(),
            Sym::Comma | Sym::B | Sym::Zero | Sym::Slash if sup => {
                (if star { "*" } else { " " }).into()
            }
            Sym::Comma => ",".into(),
            Sym::B => " ".into(),
            Sym::Zero => "0".into(),
            Sym::Slash => "/".into(),
            Sym::Dot => ".".into(),
            Sym::Plus => (if neg { "-" } else { "+" }).into(),
            Sym::Minus => (if neg { "-" } else { " " }).into(),
            Sym::Cr => (if neg { "CR" } else { "  " }).into(),
            Sym::Db => (if neg { "DB" } else { "  " }).into(),
            Sym::Cur(c) => c.to_string(),
        };
        out.push(piece);
    }
    // 浮動挿入の記号を、抑制した位置の最後に置く
    if let Some(&at) = run.iter().rev().find(|&&i| i < sig) {
        out[at] = match syms[at] {
            Sym::Cur(c) => c.to_string(),
            Sym::Plus => (if neg { "-" } else { "+" }).into(),
            _ => (if neg { "-" } else { " " }).into(),
        };
    }
    (out.concat(), overflow)
}

/// 書いた文字列を読む。全体が空白なら `Some(None)`、形が合わなければ `None`。
pub(crate) fn parse(syms: &[Sym], text: &str) -> Option<Option<Decimal>> {
    let chars: Vec<char> = text.chars().collect();
    let total: usize = syms.iter().map(|s| width(*s)).sum();
    if chars.len() != total {
        return None;
    }
    if chars.iter().all(|c| *c == ' ') {
        return Some(None);
    }
    let pos = digit_positions(syms);
    let is_digit_pos = |i: usize| pos.iter().any(|p| p.0 == i);
    let nfrac = pos.iter().filter(|p| !p.1).count() as i32;
    let mut neg = false;
    let mut v: i128 = 0;
    let mut k = 0;
    for (i, s) in syms.iter().enumerate() {
        match s {
            Sym::V | Sym::P => continue,
            Sym::Cr | Sym::Db => {
                let t: String = chars[k..k + 2].iter().collect();
                match t.as_str() {
                    "CR" | "DB" => neg = true,
                    "  " => {}
                    _ => return None,
                }
                k += 2;
                continue;
            }
            _ => {}
        }
        let c = chars[k];
        k += 1;
        if is_digit_pos(i) {
            let d = match c {
                '0'..='9' => c as u8 - b'0',
                ' ' | '*' | '+' | ',' => 0,
                '-' => {
                    neg = true;
                    0
                }
                c if matches!(s, Sym::Cur(x) if *x == c) => 0,
                _ => return None,
            };
            v = v.checked_mul(10)?.checked_add(d as i128)?;
            continue;
        }
        match (s, c) {
            (Sym::Nine, _) => return None,
            (Sym::Plus | Sym::Minus, '-') => neg = true,
            (Sym::Plus | Sym::Minus, '+' | ' ') => {}
            (Sym::Cur(x), c) if c == *x || c == ' ' => {}
            (Sym::Comma | Sym::B | Sym::Zero | Sym::Slash | Sym::Dot, _) => {}
            _ => return None,
        }
    }
    Some(Some(Decimal::new(if neg { -v } else { v }, nfrac)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Kind;

    fn syms(pic: &str) -> Vec<Sym> {
        match crate::pic::kind_of(Some(pic), &Default::default())
            .unwrap()
            .0
        {
            Kind::Edited { syms, .. } => syms,
            k => panic!("{k:?}"),
        }
    }

    fn f(pic: &str, v: &str) -> String {
        let s = syms(pic);
        let scale = digit_positions(&s).iter().filter(|p| !p.1).count() as i32;
        let d = Decimal::parse(v).unwrap().rescale(scale).unwrap();
        let (t, _) = format(&s, d, false);
        // 読み戻すと同じ値
        if d.value != 0 {
            assert_eq!(parse(&s, &t), Some(Some(d)), "{pic} {v} {t:?}");
        }
        t
    }

    #[test]
    fn edited_pictures() {
        assert_eq!(f("ZZZ9", "42"), "  42");
        assert_eq!(f("ZZZ9", "0"), "   0");
        assert_eq!(f("ZZZZ", "0"), "    ");
        assert_eq!(f("ZZ,ZZ9.99", "1234.5"), " 1,234.50");
        assert_eq!(f("ZZ,ZZ9.99", "12.5"), "    12.50");
        assert_eq!(f("ZZ,ZZ9.99-", "-12.5"), "    12.50-");
        assert_eq!(f("-ZZZ9", "-7"), "-   7");
        assert_eq!(f("+ZZZ9", "7"), "+   7");
        assert_eq!(f("$$$,$$9.99", "12.5"), "    $12.50");
        assert_eq!(f("$$$,$$9.99", "12345.67"), "$12,345.67");
        assert_eq!(f("----9", "-123"), " -123");
        assert_eq!(f("++++9", "123"), " +123");
        assert_eq!(f("***9.99", "5"), "***5.00");
        assert_eq!(f("ZZ9.99CR", "-1.5"), "  1.50CR");
        assert_eq!(f("ZZ9.99CR", "1.5"), "  1.50  ");
        assert_eq!(f("9999/99/99", "20261007"), "2026/10/07");
        assert_eq!(f("99B99", "1234"), "12 34");
        assert_eq!(f("\\\\\\,\\\\9", "1500"), " \\1,500");
        // あふれは上の桁を落とす
        let s = syms("ZZ9");
        assert_eq!(
            format(&s, Decimal::new(12345, 0), false),
            ("345".into(), true)
        );
        // BLANK WHEN ZERO
        let s = syms("ZZ9.99");
        assert_eq!(format(&s, Decimal::new(0, 2), true).0, "      ");
        // 形の合わない文字列
        assert_eq!(parse(&s, "  1.2x"), None);
        assert_eq!(parse(&s, "      "), Some(None));
    }
}
