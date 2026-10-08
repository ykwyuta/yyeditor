//! Excel の表示形式（書式記号）の解析と表示（15 章 8.1）。
//!
//! * 区分 `正;負;ゼロ;文字列`、条件 `[>=100]`、色 `[Red]`・`[赤]`・`[Color10]`
//! * 数値 `0`・`#`・`?`・`.`・`,`（桁区切り、末尾の `,` は 1000 で割る）・`%`・`E+`・`E-`・分数 `# ?/?`
//! * 文字 `"…"`・`\x`・`_x`（x の幅の空白）・`*x`（列幅まで x で埋める）・`@`
//! * 日付・時刻 `yyyy`・`yy`・`m`〜`mmmmm`・`d`〜`dddd`・`aaa`・`aaaa`・`h`・`hh`・`m`・`mm`（分）・`s`・`ss`・
//!   `.0`〜`.000`・`AM/PM`・`A/P`・`午前/午後`・`[h]`・`[mm]`・`[ss]`
//! * 和暦 `g`・`gg`・`ggg`・`e`・`ee`、ロケール `[$-411]`・`[$-ja-JP]`、通貨 `[$¥-411]`
//! * `General`（`標準`・`G/標準`）
//!
//! 数値の丸めは Excel と同じく、有効数字 15 桁に丸めてから、十進で四捨五入（0 から遠い方へ）する。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use crate::date::{DateSystem, datetime_from_serial, weekday};
use crate::general::{general, round15};

/// 色（`[Red]` など。`Index` は `[Color1]`〜`[Color56]`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FmtColor {
    Black,
    Blue,
    Cyan,
    Green,
    Magenta,
    Red,
    White,
    Yellow,
    Index(u8),
}

impl FmtColor {
    /// RGB（Excel の既定の色の一覧）。
    pub fn rgb(self) -> (u8, u8, u8) {
        match self {
            FmtColor::Black => (0, 0, 0),
            FmtColor::Blue => (0, 0, 255),
            FmtColor::Cyan => (0, 255, 255),
            FmtColor::Green => (0, 255, 0),
            FmtColor::Magenta => (255, 0, 255),
            FmtColor::Red => (255, 0, 0),
            FmtColor::White => (255, 255, 255),
            FmtColor::Yellow => (255, 255, 0),
            FmtColor::Index(i) => PALETTE[(i.clamp(1, 56) - 1) as usize],
        }
    }
}

/// Excel の既定の 56 色。
const PALETTE: [(u8, u8, u8); 56] = [
    (0, 0, 0),
    (255, 255, 255),
    (255, 0, 0),
    (0, 255, 0),
    (0, 0, 255),
    (255, 255, 0),
    (255, 0, 255),
    (0, 255, 255),
    (128, 0, 0),
    (0, 128, 0),
    (0, 0, 128),
    (128, 128, 0),
    (128, 0, 128),
    (0, 128, 128),
    (192, 192, 192),
    (128, 128, 128),
    (153, 153, 255),
    (153, 51, 102),
    (255, 255, 204),
    (204, 255, 255),
    (102, 0, 102),
    (255, 128, 128),
    (0, 102, 204),
    (204, 204, 255),
    (0, 0, 128),
    (255, 0, 255),
    (255, 255, 0),
    (0, 255, 255),
    (128, 0, 128),
    (128, 0, 0),
    (0, 128, 128),
    (0, 0, 255),
    (0, 204, 255),
    (204, 255, 255),
    (204, 255, 204),
    (255, 255, 153),
    (153, 204, 255),
    (255, 153, 204),
    (204, 153, 255),
    (255, 204, 153),
    (51, 102, 255),
    (51, 204, 204),
    (153, 204, 0),
    (255, 204, 0),
    (255, 153, 0),
    (255, 102, 0),
    (102, 102, 153),
    (150, 150, 150),
    (0, 51, 102),
    (51, 153, 102),
    (0, 51, 0),
    (51, 51, 0),
    (153, 51, 0),
    (153, 51, 102),
    (51, 51, 153),
    (51, 51, 51),
];

#[derive(Clone, Copy, Debug, PartialEq)]
enum Cmp {
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Cond {
    op: Cmp,
    value: f64,
}

impl Cond {
    fn test(&self, v: f64) -> bool {
        match self.op {
            Cmp::Lt => v < self.value,
            Cmp::Le => v <= self.value,
            Cmp::Gt => v > self.value,
            Cmp::Ge => v >= self.value,
            Cmp::Eq => v == self.value,
            Cmp::Ne => v != self.value,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Lit(String),
    /// `0`・`#`・`?`
    Digit(u8),
    Point,
    Comma,
    Percent,
    /// 指数（`E+` なら符号を常に出す）
    Exp {
        plus: bool,
    },
    Slash,
    /// `@`
    Text,
    /// `*x`
    Fill(char),
    /// `_x`
    Skip,
    Year(u8),
    Month(u8),
    Day(u8),
    /// `aaa`・`aaaa`
    WeekJa(u8),
    Hour(u8),
    Minute(u8),
    Second(u8),
    /// 秒の小数（桁数）
    Sub(u8),
    /// `AM/PM`（0）・`A/P`（1）・`午前/午後`（2）
    AmPm(u8),
    ElapsedH(u8),
    ElapsedM(u8),
    ElapsedS(u8),
    /// 元号（`g` の数）
    Era(u8),
    /// 元号の年（`e` の数）
    EraYear(u8),
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Section {
    toks: Vec<Tok>,
    color: Option<FmtColor>,
    cond: Option<Cond>,
    date: bool,
    text: bool,
    general: bool,
}

/// 解析した表示形式。
#[derive(Clone, Debug, PartialEq)]
pub struct Format {
    sections: Vec<Section>,
}

/// 表示の結果。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Formatted {
    pub text: String,
    pub color: Option<FmtColor>,
    /// `*x`: 列幅まで埋める文字と、`text` の中の位置（バイト）
    pub fill: Option<(usize, char)>,
    /// 日付の範囲外など、`#` で埋めて表示する
    pub overflow: bool,
}

/// 表示する値。
#[derive(Clone, Copy, Debug)]
pub enum FmtValue<'a> {
    Number(f64),
    Text(&'a str),
}

fn color_of(name: &str) -> Option<FmtColor> {
    let l = name.to_lowercase();
    Some(match l.as_str() {
        "black" | "黒" => FmtColor::Black,
        "blue" | "青" => FmtColor::Blue,
        "cyan" | "水" => FmtColor::Cyan,
        "green" | "緑" => FmtColor::Green,
        "magenta" | "紫" => FmtColor::Magenta,
        "red" | "赤" => FmtColor::Red,
        "white" | "白" => FmtColor::White,
        "yellow" | "黄" => FmtColor::Yellow,
        _ => {
            let n = l.strip_prefix("color").or_else(|| l.strip_prefix("色"))?;
            let i: u8 = n.trim().parse().ok()?;
            if !(1..=56).contains(&i) {
                return None;
            }
            FmtColor::Index(i)
        }
    })
}

fn cond_of(s: &str) -> Option<Cond> {
    let (op, rest) = if let Some(r) = s.strip_prefix(">=") {
        (Cmp::Ge, r)
    } else if let Some(r) = s.strip_prefix("<=") {
        (Cmp::Le, r)
    } else if let Some(r) = s.strip_prefix("<>") {
        (Cmp::Ne, r)
    } else if let Some(r) = s.strip_prefix('>') {
        (Cmp::Gt, r)
    } else if let Some(r) = s.strip_prefix('<') {
        (Cmp::Lt, r)
    } else {
        (Cmp::Eq, s.strip_prefix('=')?)
    };
    Some(Cond {
        op,
        value: rest.trim().parse().ok()?,
    })
}

/// 書式記号を区分に分ける（引用符・`\`・`[]` の中の `;` は区切りにしない）。
fn split_sections(code: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut chars = code.chars().peekable();
    while let Some(c) = chars.next() {
        let cur = out.last_mut().unwrap();
        match c {
            '"' => {
                cur.push(c);
                for d in chars.by_ref() {
                    cur.push(d);
                    if d == '"' {
                        break;
                    }
                }
            }
            '\\' | '_' | '*' => {
                cur.push(c);
                if let Some(d) = chars.next() {
                    cur.push(d);
                }
            }
            '[' => {
                cur.push(c);
                for d in chars.by_ref() {
                    cur.push(d);
                    if d == ']' {
                        break;
                    }
                }
            }
            ';' => out.push(String::new()),
            _ => cur.push(c),
        }
    }
    out
}

fn parse_section(src: &str) -> Section {
    let mut sec = Section::default();
    let t = src.trim();
    if t.eq_ignore_ascii_case("general") || t == "標準" || t == "G/標準" {
        sec.general = true;
        return sec;
    }
    let cs: Vec<char> = src.chars().collect();
    let mut i = 0;
    let mut lit = String::new();
    let flush = |lit: &mut String, toks: &mut Vec<Tok>| {
        if !lit.is_empty() {
            toks.push(Tok::Lit(std::mem::take(lit)));
        }
    };
    let mut toks = Vec::new();
    let lower = |c: char| c.to_ascii_lowercase();
    let run = |cs: &[char], i: usize, c: char| {
        cs[i..]
            .iter()
            .take_while(|&&x| x.to_ascii_lowercase() == c)
            .count()
    };
    while i < cs.len() {
        let c = cs[i];
        match c {
            '"' => {
                i += 1;
                while i < cs.len() && cs[i] != '"' {
                    lit.push(cs[i]);
                    i += 1;
                }
                i += 1;
            }
            '\\' => {
                if let Some(&d) = cs.get(i + 1) {
                    lit.push(d);
                }
                i += 2;
            }
            '_' => {
                flush(&mut lit, &mut toks);
                toks.push(Tok::Skip);
                i += 2;
            }
            '*' => {
                flush(&mut lit, &mut toks);
                if let Some(&d) = cs.get(i + 1) {
                    toks.push(Tok::Fill(d));
                }
                i += 2;
            }
            '[' => {
                let end = cs[i..].iter().position(|&x| x == ']').map(|p| i + p);
                let Some(end) = end else {
                    lit.push(c);
                    i += 1;
                    continue;
                };
                let inner: String = cs[i + 1..end].iter().collect();
                let il = inner.to_ascii_lowercase();
                if let Some(col) = color_of(&inner) {
                    sec.color = Some(col);
                } else if let Some(cd) = cond_of(&inner) {
                    sec.cond = Some(cd);
                } else if il.starts_with('h') && il.chars().all(|x| x == 'h') {
                    flush(&mut lit, &mut toks);
                    toks.push(Tok::ElapsedH(inner.len() as u8));
                    sec.date = true;
                } else if il.starts_with('m') && il.chars().all(|x| x == 'm') {
                    flush(&mut lit, &mut toks);
                    toks.push(Tok::ElapsedM(inner.len() as u8));
                    sec.date = true;
                } else if il.starts_with('s') && il.chars().all(|x| x == 's') {
                    flush(&mut lit, &mut toks);
                    toks.push(Tok::ElapsedS(inner.len() as u8));
                    sec.date = true;
                } else if let Some(rest) = inner.strip_prefix('$') {
                    // [$¥-411]: 通貨の記号は文字として出す。[$-411] はロケールだけ
                    let sym = rest.split('-').next().unwrap_or("");
                    lit.push_str(sym);
                }
                // [DBNum1] などは無視する
                i = end + 1;
            }
            '0' | '#' | '?' => {
                flush(&mut lit, &mut toks);
                toks.push(Tok::Digit(c as u8));
                i += 1;
            }
            '.' => {
                // 時刻の秒の小数（s の後の .0〜.000）
                let zeros = cs[i + 1..].iter().take_while(|&&x| x == '0').count();
                let after_sec = toks
                    .iter()
                    .rev()
                    .any(|t| matches!(t, Tok::Second(_) | Tok::ElapsedS(_)));
                if sec.date && after_sec && zeros > 0 {
                    flush(&mut lit, &mut toks);
                    toks.push(Tok::Sub(zeros.min(3) as u8));
                    i += 1 + zeros;
                } else {
                    flush(&mut lit, &mut toks);
                    toks.push(Tok::Point);
                    i += 1;
                }
            }
            ',' => {
                flush(&mut lit, &mut toks);
                toks.push(Tok::Comma);
                i += 1;
            }
            '%' => {
                flush(&mut lit, &mut toks);
                toks.push(Tok::Percent);
                i += 1;
            }
            '/' if !sec.date && toks.iter().any(|t| matches!(t, Tok::Digit(_))) => {
                flush(&mut lit, &mut toks);
                toks.push(Tok::Slash);
                i += 1;
            }
            'E' | 'e'
                if !sec.date
                    && matches!(cs.get(i + 1), Some('+') | Some('-'))
                    && toks.iter().any(|t| matches!(t, Tok::Digit(_))) =>
            {
                flush(&mut lit, &mut toks);
                toks.push(Tok::Exp {
                    plus: cs[i + 1] == '+',
                });
                i += 2;
            }
            '@' => {
                flush(&mut lit, &mut toks);
                toks.push(Tok::Text);
                sec.text = true;
                i += 1;
            }
            _ => {
                let l = lower(c);
                let upper: String = cs[i..]
                    .iter()
                    .take(5)
                    .collect::<String>()
                    .to_ascii_uppercase();
                if upper.starts_with("AM/PM") {
                    flush(&mut lit, &mut toks);
                    toks.push(Tok::AmPm(0));
                    sec.date = true;
                    i += 5;
                } else if upper.starts_with("A/P") {
                    flush(&mut lit, &mut toks);
                    toks.push(Tok::AmPm(1));
                    sec.date = true;
                    i += 3;
                } else if cs[i..].starts_with(&['午', '前', '/', '午', '後']) {
                    flush(&mut lit, &mut toks);
                    toks.push(Tok::AmPm(2));
                    sec.date = true;
                    i += 5;
                } else if matches!(l, 'y' | 'm' | 'd' | 'h' | 's' | 'g' | 'e' | 'a') {
                    let n = run(&cs, i, l);
                    // 'a' は aaa・aaaa（日本語の曜日）だけ
                    if l == 'a' && n < 3 {
                        lit.push(c);
                        i += 1;
                        continue;
                    }
                    flush(&mut lit, &mut toks);
                    let n8 = n.min(5) as u8;
                    toks.push(match l {
                        'y' => Tok::Year(if n <= 2 { 2 } else { 4 }),
                        'm' => Tok::Month(n8),
                        'd' => Tok::Day(n8.min(4)),
                        'h' => Tok::Hour(n8.min(2)),
                        's' => Tok::Second(n8.min(2)),
                        'g' => Tok::Era(n8.min(3)),
                        'e' => Tok::EraYear(n8.min(2)),
                        _ => Tok::WeekJa(n8.min(4)),
                    });
                    sec.date = true;
                    i += n;
                } else {
                    lit.push(c);
                    i += 1;
                }
            }
        }
    }
    flush(&mut lit, &mut toks);
    // 日付の書式では , . % は文字
    if sec.date {
        for t in toks.iter_mut() {
            match t {
                Tok::Comma => *t = Tok::Lit(",".into()),
                Tok::Point => *t = Tok::Lit(".".into()),
                Tok::Percent => *t = Tok::Lit("%".into()),
                Tok::Digit(d) => *t = Tok::Lit((*d as char).to_string()),
                _ => {}
            }
        }
    }
    // m の分・月の判別: h の後、または s の前の m は分
    if sec.date {
        let n = toks.len();
        for k in 0..n {
            if let Tok::Month(w) = toks[k]
                && w <= 2
            {
                let prev = toks[..k]
                    .iter()
                    .rev()
                    .find(|t| !matches!(t, Tok::Lit(_) | Tok::Skip | Tok::Fill(_)));
                let next = toks[k + 1..]
                    .iter()
                    .find(|t| !matches!(t, Tok::Lit(_) | Tok::Skip | Tok::Fill(_)));
                if matches!(prev, Some(Tok::Hour(_)) | Some(Tok::ElapsedH(_)))
                    || matches!(next, Some(Tok::Second(_)) | Some(Tok::ElapsedS(_)))
                {
                    toks[k] = Tok::Minute(w);
                }
            }
        }
    }
    sec.toks = toks;
    sec
}

impl Format {
    pub fn parse(code: &str) -> Format {
        let mut sections: Vec<Section> = split_sections(code)
            .iter()
            .map(|s| parse_section(s))
            .collect();
        if sections.is_empty() {
            sections.push(Section {
                general: true,
                ..Section::default()
            });
        }
        Format { sections }
    }

    pub fn is_general(&self) -> bool {
        self.sections.len() == 1 && self.sections[0].general
    }

    /// 日付・時刻の書式か（数値の区分のどれかに日付・時刻の記号がある）。
    pub fn is_date(&self) -> bool {
        self.sections.iter().any(|s| s.date)
    }

    /// 文字列の区分。
    fn text_section(&self) -> Option<&Section> {
        if self.sections.len() >= 4 {
            return Some(&self.sections[3]);
        }
        self.sections
            .iter()
            .find(|s| s.text && !s.toks.iter().any(|t| matches!(t, Tok::Digit(_))))
    }

    /// 値を表示する。
    pub fn format(&self, v: FmtValue<'_>, sys: DateSystem) -> Formatted {
        match v {
            FmtValue::Text(s) => match self.text_section() {
                Some(sec) => {
                    let mut out = Formatted {
                        color: sec.color,
                        ..Formatted::default()
                    };
                    for t in &sec.toks {
                        match t {
                            Tok::Text => out.text.push_str(s),
                            Tok::Lit(l) => out.text.push_str(l),
                            Tok::Skip => out.text.push(' '),
                            Tok::Fill(c) => out.fill = Some((out.text.len(), *c)),
                            _ => {}
                        }
                    }
                    out
                }
                None => Formatted {
                    text: s.to_owned(),
                    ..Formatted::default()
                },
            },
            FmtValue::Number(n) => self.format_number(n, sys),
        }
    }

    fn format_number(&self, n: f64, sys: DateSystem) -> Formatted {
        // 区分を選ぶ
        let secs: Vec<&Section> = self
            .sections
            .iter()
            .filter(|s| {
                !(s.text
                    && !s.toks.iter().any(|t| matches!(t, Tok::Digit(_)))
                    && self.sections.len() < 4)
            })
            .collect();
        let numeric: Vec<&Section> = if self.sections.len() >= 4 {
            self.sections[..3].iter().collect()
        } else {
            secs
        };
        let has_cond = numeric.iter().any(|s| s.cond.is_some());
        let (sec, negate) = if has_cond {
            // 条件の区分: 合うものを順に。最後の区分は残り
            let mut chosen = None;
            for s in &numeric {
                if let Some(c) = s.cond
                    && c.test(n)
                {
                    chosen = Some(*s);
                    break;
                }
            }
            match chosen {
                Some(s) => (s, false),
                None => {
                    let last = numeric.iter().rev().find(|s| s.cond.is_none());
                    match last {
                        Some(s) => (*s, n < 0.0 && numeric.len() <= 2),
                        None => {
                            return Formatted {
                                overflow: true,
                                ..Formatted::default()
                            };
                        }
                    }
                }
            }
        } else {
            match numeric.len() {
                0 => {
                    return Formatted {
                        text: general(n),
                        ..Formatted::default()
                    };
                }
                1 => (numeric[0], false),
                2 => {
                    if n < 0.0 {
                        (numeric[1], true)
                    } else {
                        (numeric[0], false)
                    }
                }
                _ => {
                    if n > 0.0 {
                        (numeric[0], false)
                    } else if n < 0.0 {
                        (numeric[1], true)
                    } else {
                        (numeric[2], false)
                    }
                }
            }
        };
        let v = if negate { -n } else { n };
        let mut out = Formatted {
            color: sec.color,
            ..Formatted::default()
        };
        if sec.general {
            out.text = general(v);
            return out;
        }
        if sec.date {
            if !format_date(sec, v, sys, &mut out) {
                out.overflow = true;
                out.text.clear();
            }
            return out;
        }
        format_num(sec, v, &mut out);
        out
    }
}

// ---- 数値 ----------------------------------------------------------------------------

/// 十進の数字列（`digits` × 10^(exp+1-len)）を小数点以下 `dec` 桁に四捨五入して、整数部と小数部の
/// 数字を返す。
fn decimal_digits(v: f64, dec: usize) -> (String, String) {
    if v == 0.0 {
        return ("0".into(), "0".repeat(dec));
    }
    let (_, digits, exp) = round15(v);
    // 数字の並び（小数点の位置 = exp + 1）
    let point = exp + 1;
    let mut all: Vec<u8> = digits.iter().map(|d| d - b'0').collect();
    // 小数点の位置まで前に 0 を足す
    let mut point_idx = point;
    if point_idx < 0 {
        let pad = (-point_idx) as usize;
        let mut v = vec![0u8; pad];
        v.extend(all);
        all = v;
        point_idx = 0;
    }
    let point_idx = point_idx as usize;
    while all.len() < point_idx + dec {
        all.push(0);
    }
    // 丸め（dec 桁の次の数字で、0 から遠い方へ）
    let keep = point_idx + dec;
    let round_up = all.get(keep).is_some_and(|&d| d >= 5);
    all.truncate(keep);
    let mut carry = round_up;
    let mut k = all.len();
    while carry && k > 0 {
        k -= 1;
        if all[k] == 9 {
            all[k] = 0;
        } else {
            all[k] += 1;
            carry = false;
        }
    }
    let mut int_len = point_idx;
    if carry {
        all.insert(0, 1);
        int_len += 1;
    }
    let int: String = all[..int_len].iter().map(|d| (b'0' + d) as char).collect();
    let frac: String = all[int_len..].iter().map(|d| (b'0' + d) as char).collect();
    let int = int.trim_start_matches('0').to_string();
    (int, frac)
}

fn format_num(sec: &Section, v: f64, out: &mut Formatted) {
    let toks = &sec.toks;
    // 構造: 整数部の数字・小数部の数字・指数部の数字・分数
    let point = toks.iter().position(|t| *t == Tok::Point);
    let exp = toks.iter().position(|t| matches!(t, Tok::Exp { .. }));
    let slash = toks.iter().position(|t| *t == Tok::Slash);
    let percent = toks.iter().filter(|t| **t == Tok::Percent).count();
    let digit_positions: Vec<usize> = toks
        .iter()
        .enumerate()
        .filter(|(_, t)| matches!(t, Tok::Digit(_)))
        .map(|(i, _)| i)
        .collect();
    let last_digit = digit_positions.last().copied();
    // 数字の直後の , の連なり（と、小数点の直前の , の連なり）は 1000 で割る。それ以外の、整数部の
    // 数字の間の , は桁区切り
    let mut scale = 0;
    let mut grouping = false;
    if let Some(ld) = last_digit {
        let int_last = digit_positions
            .iter()
            .copied()
            .rfind(|&i| point.is_none_or(|p| i < p) && exp.is_none_or(|e| i < e));
        let mut k = ld + 1;
        while toks.get(k) == Some(&Tok::Comma) {
            scale += 1;
            k += 1;
        }
        if let Some(p) = point {
            let mut m = p;
            while m > 0 && toks[m - 1] == Tok::Comma {
                scale += 1;
                m -= 1;
            }
        }
        if let Some(il) = int_last {
            grouping = toks[digit_positions[0]..il].contains(&Tok::Comma);
        }
    }
    let mut x = v.abs() * 100f64.powi(percent as i32) / 1000f64.powi(scale);
    let neg = v < 0.0;
    if let Some(si) = slash {
        format_fraction(sec, x, neg, si, out);
        return;
    }
    let int_digits: Vec<u8> = toks[..point.or(exp).unwrap_or(toks.len())]
        .iter()
        .filter_map(|t| {
            if let Tok::Digit(d) = t {
                Some(*d)
            } else {
                None
            }
        })
        .collect();
    let frac_digits: Vec<u8> = match point {
        Some(p) => toks[p + 1..exp.unwrap_or(toks.len())]
            .iter()
            .filter_map(|t| {
                if let Tok::Digit(d) = t {
                    Some(*d)
                } else {
                    None
                }
            })
            .collect(),
        None => Vec::new(),
    };
    let mut exponent = 0i32;
    if exp.is_some() && x != 0.0 {
        // 仮数の整数部の桁数に合わせる（##0.0E+0 は 3 桁ごとの工学表記）
        let n_int = int_digits.len().max(1) as i32;
        let e = x.log10().floor() as i32;
        exponent = if n_int > 1 && int_digits.contains(&b'#') {
            e.div_euclid(n_int) * n_int
        } else {
            e - (n_int - 1)
        };
        x /= 10f64.powi(exponent);
        // 丸めで 10 になったら直す
        let (i, _) = decimal_digits(x, frac_digits.len());
        if i.len() as i32 > n_int.max(1) && !(n_int > 1 && int_digits.contains(&b'#')) {
            exponent += 1;
            x /= 10.0;
        }
    }
    let (int_s, frac_s) = decimal_digits(x, frac_digits.len());
    // 整数部の数字を埋める
    let need = int_digits.len();
    let mut int_chars: Vec<char> = int_s.chars().collect();
    // 必要な桁（0 は 0、? は空白、# は省く）
    let mut filled: Vec<char> = Vec::new();
    if int_chars.len() < need {
        let missing = need - int_chars.len();
        for &d in &int_digits[..missing] {
            match d {
                b'0' => filled.push('0'),
                b'?' => filled.push(' '),
                _ => {}
            }
        }
    }
    filled.append(&mut int_chars);
    // 桁区切り
    let int_text: String = if grouping {
        let digits: Vec<char> = filled
            .iter()
            .copied()
            .filter(|c| c.is_ascii_digit())
            .collect();
        let lead: String = filled.iter().take_while(|c| !c.is_ascii_digit()).collect();
        let mut g = String::new();
        for (i, c) in digits.iter().enumerate() {
            if i > 0 && (digits.len() - i) % 3 == 0 {
                g.push(',');
            }
            g.push(*c);
        }
        lead + &g
    } else {
        filled.into_iter().collect()
    };
    // 小数部（末尾の # は省く、? は空白）
    let mut frac_text: Vec<char> = frac_s.chars().collect();
    let mut k = frac_text.len();
    while k > 0 {
        let d = frac_digits[k - 1];
        if frac_text[k - 1] == '0' && d != b'0' {
            if d == b'?' {
                frac_text[k - 1] = ' ';
            } else {
                frac_text.pop();
            }
            k -= 1;
        } else {
            break;
        }
    }
    let frac_text: String = frac_text.into_iter().collect();
    // 符号（1 つ目の区分で負の値、または区分が 1 つのとき）
    // 数字のない区分（"負" など）は符号も出さない
    if neg && first_digit_exists(toks) {
        out.text.push('-');
    }
    // 組み立て: 最初の数字の位置に整数部、小数点に小数部、指数
    let first_digit = digit_positions.first().copied();
    let mut int_done = false;
    let mut frac_done = false;
    for (i, t) in toks.iter().enumerate() {
        match t {
            Tok::Lit(l) => out.text.push_str(l),
            Tok::Skip => out.text.push(' '),
            Tok::Fill(c) => out.fill = Some((out.text.len(), *c)),
            Tok::Percent => out.text.push('%'),
            Tok::Text => {}
            Tok::Digit(_) => {
                let in_int = point.is_none_or(|p| i < p) && exp.is_none_or(|e| i < e);
                if in_int {
                    if !int_done && Some(i) >= first_digit {
                        out.text.push_str(&int_text);
                        int_done = true;
                    }
                } else if point.is_some_and(|p| i > p) && exp.is_none_or(|e| i < e) && !frac_done {
                    out.text.push_str(&frac_text);
                    frac_done = true;
                }
            }
            Tok::Point => {
                if !int_done {
                    out.text.push_str(&int_text);
                    int_done = true;
                }
                out.text.push('.');
            }
            Tok::Exp { plus } => {
                out.text.push('E');
                if exponent < 0 {
                    out.text.push('-');
                } else if *plus {
                    out.text.push('+');
                }
                let n_exp = toks[i + 1..]
                    .iter()
                    .filter(|t| matches!(t, Tok::Digit(_)))
                    .count()
                    .max(1);
                out.text
                    .push_str(&format!("{:0width$}", exponent.abs(), width = n_exp));
                break;
            }
            Tok::Comma => {}
            _ => {}
        }
    }
    if !int_done && first_digit.is_some() {
        out.text.push_str(&int_text);
    }
    // 指数の後の文字
    if let Some(e) = exp {
        for t in toks[e + 1..].iter() {
            if let Tok::Lit(l) = t {
                out.text.push_str(l);
            }
        }
    }
}

fn first_digit_exists(toks: &[Tok]) -> bool {
    toks.iter().any(|t| matches!(t, Tok::Digit(_)))
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// 分数（`# ?/?`・`# ??/16`・`?/?`）。
fn format_fraction(sec: &Section, x: f64, neg: bool, si: usize, out: &mut Formatted) {
    let toks = &sec.toks;
    // 分子の前に整数部があるか（数字・空白の文字・数字）
    let num_start = toks[..si]
        .iter()
        .rposition(|t| !matches!(t, Tok::Digit(_)))
        .map_or(0, |p| p + 1);
    let has_int = toks[..num_start].iter().any(|t| matches!(t, Tok::Digit(_)));
    let num_digits = si - num_start;
    let den_toks: Vec<&Tok> = toks[si + 1..]
        .iter()
        .take_while(|t| matches!(t, Tok::Digit(_) | Tok::Lit(_)))
        .collect();
    let fixed: Option<u64> = den_toks.iter().find_map(|t| match t {
        Tok::Lit(l) => l.trim().parse().ok(),
        _ => None,
    });
    let den_digits = den_toks
        .iter()
        .filter(|t| matches!(t, Tok::Digit(_)))
        .count()
        .max(1);
    let (ip, frac) = if has_int {
        (x.trunc(), x.fract())
    } else {
        (0.0, x)
    };
    let (mut num, mut den) = match fixed {
        Some(d) => ((frac * d as f64).round() as u64, d),
        None => {
            let max_den = 10u64.pow(den_digits as u32) - 1;
            let mut best = (0u64, 1u64, f64::INFINITY);
            for d in 1..=max_den {
                let n = (frac * d as f64).round();
                let err = (frac - n / d as f64).abs();
                if err < best.2 - 1e-12 {
                    best = (n as u64, d, err);
                }
            }
            let g = gcd(best.0, best.1).max(1);
            (best.0 / g, best.1 / g)
        }
    };
    let mut ip = ip as u64;
    if num == den && den != 0 && has_int {
        ip += 1;
        num = 0;
        den = den.max(1);
    }
    if neg {
        out.text.push('-');
    }
    if has_int {
        if ip > 0 || num == 0 {
            out.text.push_str(&ip.to_string());
        }
        if num != 0 {
            if ip > 0 {
                out.text.push(' ');
            }
        } else {
            return;
        }
    }
    let ns = num.to_string();
    for _ in ns.len()..num_digits {
        out.text.push(' ');
    }
    out.text.push_str(&ns);
    out.text.push('/');
    let ds = den.to_string();
    out.text.push_str(&ds);
    if fixed.is_none() {
        for _ in ds.len()..den_digits {
            out.text.push(' ');
        }
    }
}

// ---- 日付 ----------------------------------------------------------------------------

const MONTHS: [&str; 12] = [
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
];
const DAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];
const DAYS_JA: [&str; 7] = ["日", "月", "火", "水", "木", "金", "土"];

/// 元号（始まりの年月日、名前、略称、ローマ字）。
type Era = ((i32, u32, u32), &'static str, &'static str, &'static str);
const ERAS: [Era; 5] = [
    ((1868, 1, 1), "明治", "明", "M"),
    ((1912, 7, 30), "大正", "大", "T"),
    ((1926, 12, 25), "昭和", "昭", "S"),
    ((1989, 1, 8), "平成", "平", "H"),
    ((2019, 5, 1), "令和", "令", "R"),
];

fn era_of(y: i32, m: u32, d: u32) -> Option<(usize, i32)> {
    let i = ERAS.iter().rposition(|(start, ..)| (y, m, d) >= *start)?;
    Some((i, y - ERAS[i].0.0 + 1))
}

fn format_date(sec: &Section, v: f64, sys: DateSystem, out: &mut Formatted) -> bool {
    if v < 0.0 {
        return false;
    }
    let has_sub = sec
        .toks
        .iter()
        .find_map(|t| if let Tok::Sub(n) = t { Some(*n) } else { None });
    // 秒の小数を出さないときは秒に丸める
    let unit = match has_sub {
        Some(n) => 86_400.0 * 10f64.powi(n as i32),
        None => 86_400.0,
    };
    let rounded = (v * unit).round() / unit;
    let Some(dt) = datetime_from_serial(sys, rounded) else {
        return false;
    };
    let ampm = sec.toks.iter().any(|t| matches!(t, Tok::AmPm(_)));
    let total_secs = (rounded * 86_400.0).round();
    for t in &sec.toks {
        match t {
            Tok::Lit(l) => out.text.push_str(l),
            Tok::Skip => out.text.push(' '),
            Tok::Fill(c) => out.fill = Some((out.text.len(), *c)),
            Tok::Year(2) => out.text.push_str(&format!("{:02}", dt.year % 100)),
            Tok::Year(_) => out.text.push_str(&format!("{:04}", dt.year)),
            Tok::Month(1) => out.text.push_str(&dt.month.to_string()),
            Tok::Month(2) => out.text.push_str(&format!("{:02}", dt.month)),
            Tok::Month(3) => out
                .text
                .push_str(&MONTHS[(dt.month.max(1) - 1) as usize][..3]),
            Tok::Month(4) => out.text.push_str(MONTHS[(dt.month.max(1) - 1) as usize]),
            Tok::Month(_) => out
                .text
                .push_str(&MONTHS[(dt.month.max(1) - 1) as usize][..1]),
            Tok::Day(1) => out.text.push_str(&dt.day.to_string()),
            Tok::Day(2) => out.text.push_str(&format!("{:02}", dt.day)),
            Tok::Day(n) => {
                let w = weekday(sys, rounded).unwrap_or(0) as usize;
                out.text
                    .push_str(if *n == 3 { &DAYS[w][..3] } else { DAYS[w] });
            }
            Tok::WeekJa(n) => {
                let w = weekday(sys, rounded).unwrap_or(0) as usize;
                out.text.push_str(DAYS_JA[w]);
                if *n >= 4 {
                    out.text.push_str("曜日");
                }
            }
            Tok::Hour(n) => {
                let h = if ampm {
                    let h = dt.hour % 12;
                    if h == 0 { 12 } else { h }
                } else {
                    dt.hour
                };
                out.text.push_str(&if *n >= 2 {
                    format!("{h:02}")
                } else {
                    h.to_string()
                });
            }
            Tok::Minute(n) => out.text.push_str(&if *n >= 2 {
                format!("{:02}", dt.minute)
            } else {
                dt.minute.to_string()
            }),
            Tok::Second(n) => out.text.push_str(&if *n >= 2 {
                format!("{:02}", dt.second)
            } else {
                dt.second.to_string()
            }),
            Tok::Sub(n) => {
                let frac = dt.milli as f64 / 1000.0;
                let s = format!("{:.*}", *n as usize, frac);
                out.text.push_str(s.trim_start_matches('0'));
            }
            Tok::AmPm(k) => {
                let pm = dt.hour >= 12;
                out.text.push_str(match (k, pm) {
                    (0, false) => "AM",
                    (0, true) => "PM",
                    (1, false) => "A",
                    (1, true) => "P",
                    (_, false) => "午前",
                    (_, true) => "午後",
                });
            }
            Tok::ElapsedH(n) => out.text.push_str(&format!(
                "{:0w$}",
                (total_secs / 3600.0).floor() as i64,
                w = *n as usize
            )),
            Tok::ElapsedM(n) => {
                let has_h = sec.toks.iter().any(|t| matches!(t, Tok::ElapsedH(_)));
                let m = if has_h {
                    (total_secs / 60.0).floor() as i64 % 60
                } else {
                    (total_secs / 60.0).floor() as i64
                };
                out.text.push_str(&format!("{:0w$}", m, w = *n as usize));
            }
            Tok::ElapsedS(n) => {
                out.text
                    .push_str(&format!("{:0w$}", total_secs as i64, w = *n as usize))
            }
            Tok::Era(n) => {
                let Some((i, _)) = era_of(dt.year, dt.month, dt.day) else {
                    return false;
                };
                out.text.push_str(match n {
                    1 => ERAS[i].3,
                    2 => ERAS[i].2,
                    _ => ERAS[i].1,
                });
            }
            Tok::EraYear(n) => {
                let Some((_, y)) = era_of(dt.year, dt.month, dt.day) else {
                    return false;
                };
                out.text.push_str(&if *n >= 2 {
                    format!("{y:02}")
                } else {
                    y.to_string()
                });
            }
            Tok::Digit(_) | Tok::Point | Tok::Comma | Tok::Percent => {}
            _ => {}
        }
    }
    true
}

// ---- 列幅に合わせた標準 --------------------------------------------------------------

/// 列幅（半角の文字数）に収まる「標準」の表示（Excel と同じく、丸めて収まらなければ指数表記、それでも
/// 収まらなければ `None`＝`#` で埋める）。
pub fn general_fit(v: f64, width: usize) -> Option<String> {
    let s = general(v);
    if s.len() <= width {
        return Some(s);
    }
    let a = v.abs();
    let sign = usize::from(v < 0.0);
    // 小数を丸めて収める（整数部が収まるとき）
    if (1e-4..1e11).contains(&a) {
        let int_len = (a.trunc() as u64).to_string().len() + sign;
        if int_len <= width {
            let dec = width.saturating_sub(int_len + 1);
            let t = format!("{:.*}", dec, v);
            let t = if t.contains('.') {
                t.trim_end_matches('0').trim_end_matches('.').to_string()
            } else {
                t
            };
            if t.len() <= width && t != "-0" {
                return Some(t);
            }
        }
    }
    // 指数表記
    for dec in (0..=10usize).rev() {
        let t = format!("{:.*E}", dec, v);
        let (m, e) = t.split_once('E').unwrap();
        let e: i32 = e.parse().unwrap();
        let m = if m.contains('.') {
            m.trim_end_matches('0').trim_end_matches('.')
        } else {
            m
        };
        let t = format!("{m}E{}{:02}", if e < 0 { '-' } else { '+' }, e.abs());
        if t.len() <= width {
            return Some(t);
        }
    }
    None
}

// ---- キャッシュ ----------------------------------------------------------------------

/// 解析した書式記号のキャッシュ。
pub fn parsed(code: &str) -> Arc<Format> {
    static C: OnceLock<Mutex<HashMap<String, Arc<Format>>>> = OnceLock::new();
    let m = C.get_or_init(Default::default);
    let mut g = m.lock().unwrap();
    if let Some(f) = g.get(code) {
        return f.clone();
    }
    let f = Arc::new(Format::parse(code));
    if g.len() > 4096 {
        g.clear();
    }
    g.insert(code.to_owned(), f.clone());
    f
}

/// 数値に表示形式を当てた文字列（色・埋める文字は使わない。`#` で埋める場合は `#` を 1 つ）。
pub fn format_number(code: &str, v: f64, sys: DateSystem) -> String {
    let f = parsed(code);
    let r = f.format(FmtValue::Number(v), sys);
    if r.overflow { "#".repeat(8) } else { r.text }
}

#[cfg(test)]
mod tests;
