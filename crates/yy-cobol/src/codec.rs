//! 項目のバイト列と値の変換。
//!
//! - 英数字（`PIC X`）: 文字コードの文字列。読むときは末尾の空白を除き（`JUSTIFIED` なら先頭も）、
//!   書くときは空白で埋める。長すぎれば文字の境目（EBCDIC の 2 バイト部は SI を含めて）で切る。
//!   EBCDIC の漢字を含む CCSID では SO / SI で切り替える。表にないバイトはエスケープ文字にするので、
//!   同じ文字コードで書けば元のバイトに戻る。
//! - 2 バイト文字（`PIC G`・`PIC N`）: SO / SI なしの 2 バイト文字。半角は全角にして書き、2 バイトの
//!   空白で埋める。`NATIONAL` は UTF-16（ビッグエンディアン）。
//! - ゾーン 10 進数: EBCDIC は数字 `F0`〜`F9`、符号は最後（`SIGN LEADING` なら最初）の桁のゾーン
//!   （`C`・`F` は正、`D` は負）。MS932 は数字 `30`〜`39`、負の符号は `70`〜`79`（IBM COBOL の ASCII の
//!   既定）。読むときは `{`・`A`〜`I`（正）・`}`・`J`〜`R`（負）も受け付ける。`SEPARATE` は `+`・`-` の文字。
//! - パック 10 進数: 最後の半バイトが符号（`C`・`A`・`E`・`F` は正、`D`・`B` は負。書くときは符号付きなら
//!   `C`・`D`、符号なしなら `F`）。
//! - 2 進数: 2・4・8 バイトの 2 の補数（既定はビッグエンディアン）。`COMP`・`BINARY` は `PIC` の桁数を
//!   超える値をあふれとして下の桁だけにする（`TRUNC(STD)`）、`COMP-5` は型の範囲まで。
//! - 浮動小数点: EBCDIC は IBM の 16 進浮動小数点、MS932 は IEEE 754。
//! - 数値として読めない項目（不正な数字・符号）は [`Decoded::Invalid`]。呼ぶ側は [`hex_text`] で
//!   `X'12345F'` の形の文字列にして持ち、書くときにその文字列なら元のバイトをそのまま書く。

use yy_encoding::{Ccsid, EbcdicCode, Encoding, EscapeMode, unescape_char};

use crate::edit;
use crate::num::{Decimal, f64_to_hfp, hfp_to_f64};
use crate::{Field, Kind, SignPos};

/// 固定長ファイルの文字コード。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Charset {
    /// Windows の Shift_JIS（CP932）
    Ms932,
    Ebcdic(Ccsid),
}

impl Charset {
    pub fn all() -> Vec<Charset> {
        std::iter::once(Charset::Ms932)
            .chain(Ccsid::ALL.iter().map(|c| Charset::Ebcdic(*c)))
            .collect()
    }

    pub fn name(self) -> &'static str {
        match self {
            Charset::Ms932 => "MS932",
            Charset::Ebcdic(c) => c.name(),
        }
    }

    /// 一覧に出す説明。
    pub fn label(self) -> String {
        match self {
            Charset::Ms932 => "MS932（Windows の Shift_JIS）".into(),
            Charset::Ebcdic(c) => format!("{}（{}）", c.name(), c.description()),
        }
    }

    pub fn from_name(s: &str) -> Option<Charset> {
        let t = s.trim().to_ascii_uppercase().replace('_', "-");
        if matches!(
            t.as_str(),
            "MS932" | "CP932" | "WINDOWS-31J" | "SHIFT-JIS" | "SJIS"
        ) {
            return Some(Charset::Ms932);
        }
        let n = t
            .trim_start_matches("IBM-")
            .trim_start_matches("IBM")
            .trim_start_matches("CP")
            .trim_start_matches("CCSID");
        n.trim_start_matches('-')
            .parse()
            .ok()
            .and_then(Ccsid::from_number)
            .map(Charset::Ebcdic)
    }

    pub fn is_ebcdic(self) -> bool {
        matches!(self, Charset::Ebcdic(_))
    }

    /// 1 バイトの空白。
    pub fn space(self) -> u8 {
        if self.is_ebcdic() { 0x40 } else { 0x20 }
    }

    /// 2 バイトの空白。
    fn dbcs_space(self) -> [u8; 2] {
        if self.is_ebcdic() {
            [0x40, 0x40]
        } else {
            [0x81, 0x40]
        }
    }

    fn plus_minus(self) -> (u8, u8) {
        if self.is_ebcdic() {
            (0x4E, 0x60)
        } else {
            (b'+', b'-')
        }
    }
}

/// 読んだ値。
#[derive(Clone, Debug, PartialEq)]
pub enum Decoded {
    /// 空白だけ（英数字・ゾーン 10 進数・数字編集）
    Empty,
    Num(Decimal),
    Float(f64),
    Text(String),
    /// 数値として読めないバイト列
    Invalid,
}

/// 書く値（セルの値）。
#[derive(Clone, Copy, Debug)]
pub enum Input<'a> {
    Empty,
    Number(f64),
    Text(&'a str),
    Bool(bool),
    /// エラー値（空として書く）
    Error,
}

/// 書いたときの注意の数。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Issues {
    /// 数値の桁あふれ（上の桁を落とした）
    pub overflow: u64,
    /// 文字列が長すぎて切った
    pub truncated: u64,
    /// 文字コードにない文字（`?`・`？` にした）
    pub unencodable: u64,
    /// 数値の項目に数値として読めない値（0 にした）
    pub not_number: u64,
    /// 符号なしの項目に負の値（絶対値にした）
    pub negative: u64,
}

impl Issues {
    pub fn total(&self) -> u64 {
        self.overflow + self.truncated + self.unencodable + self.not_number + self.negative
    }

    pub fn add(&mut self, o: &Issues) {
        self.overflow += o.overflow;
        self.truncated += o.truncated;
        self.unencodable += o.unencodable;
        self.not_number += o.not_number;
        self.negative += o.negative;
    }
}

/// バイト列を `X'12345F'` の形に。
pub fn hex_text(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2 + 3);
    s.push_str("X'");
    for x in b {
        s.push_str(&format!("{x:02X}"));
    }
    s.push('\'');
    s
}

/// `X'…'` の形の文字列なら、その `len` バイト。
pub fn parse_hex(s: &str, len: usize) -> Option<Vec<u8>> {
    let t = s.trim();
    let body = t
        .strip_prefix("X'")
        .or_else(|| t.strip_prefix("x'"))?
        .strip_suffix('\'')?;
    if body.len() != len * 2 {
        return None;
    }
    (0..len)
        .map(|i| u8::from_str_radix(&body[i * 2..i * 2 + 2], 16).ok())
        .collect()
}

/// 項目の読み書き。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Codec {
    pub charset: Charset,
    /// 2 進数・浮動小数点をリトルエンディアンで（Windows の `COMP-5` など）
    pub little_endian: bool,
}

/// 半角を全角に（2 バイト文字の項目に書くとき）。
fn to_fullwidth(c: char) -> char {
    match c {
        ' ' => '\u{3000}',
        '!'..='~' => char::from_u32(c as u32 - 0x21 + 0xFF01).unwrap_or(c),
        _ => c,
    }
}

/// EBCDIC の CCSID ごとの ASCII の文字の 1 バイトの符号（0 は 1 バイトでない・表にない）。
fn ascii_table(c: Ccsid) -> &'static [u8; 128] {
    use std::sync::OnceLock;
    static TABLES: [OnceLock<[u8; 128]>; Ccsid::ALL.len()] =
        [const { OnceLock::new() }; Ccsid::ALL.len()];
    let i = Ccsid::ALL.iter().position(|x| *x == c).unwrap_or(0);
    TABLES[i].get_or_init(|| {
        let mut t = [0u8; 128];
        for (b, slot) in t.iter_mut().enumerate() {
            if let Some(EbcdicCode::Single(x)) = c.encode_char(b as u8 as char) {
                *slot = x;
            }
        }
        t
    })
}

/// Shift_JIS の先頭バイトか。
fn sjis_lead(b: u8) -> bool {
    matches!(b, 0x81..=0x9F | 0xE0..=0xFC)
}

impl Codec {
    pub fn new(charset: Charset) -> Codec {
        Codec {
            charset,
            little_endian: false,
        }
    }

    // ---- 文字列 ----

    fn decode_text(&self, b: &[u8], dbcs_only: bool) -> String {
        match self.charset {
            Charset::Ms932 => {
                if b.is_ascii() {
                    // ASCII はそのまま（制御文字も）
                    return b.iter().map(|&x| x as char).collect();
                }
                let (u, _) = yy_encoding::decode_all(Encoding::Cp932, b, true);
                String::from_utf8(u).unwrap_or_default()
            }
            Charset::Ebcdic(c) => c.decode_field(b, dbcs_only),
        }
    }

    /// 文字列を `len` バイトに書く。
    fn encode_text(
        &self,
        s: &str,
        out: &mut [u8],
        dbcs_only: bool,
        justified: bool,
        issues: &mut Issues,
    ) {
        let len = out.len();
        let mut bytes: Vec<u8> = Vec::with_capacity(len);
        let mut truncated = false;
        match self.charset {
            Charset::Ms932 => {
                let conv: String;
                let s = if dbcs_only {
                    conv = s.chars().map(to_fullwidth).collect();
                    conv.as_str()
                } else {
                    s
                };
                let enc = if s.is_ascii() {
                    s.as_bytes().to_vec()
                } else {
                    match yy_encoding::encode_all(
                        Encoding::Cp932,
                        s.as_bytes(),
                        EscapeMode::Restore,
                    ) {
                        Ok(b) => b,
                        Err(_) => {
                            // 1 文字ずつ（変換できない文字は ? にする）
                            let mut b = Vec::new();
                            for ch in s.chars() {
                                let mut t = [0u8; 4];
                                match yy_encoding::encode_all(
                                    Encoding::Cp932,
                                    ch.encode_utf8(&mut t).as_bytes(),
                                    EscapeMode::Restore,
                                ) {
                                    Ok(x) => b.extend_from_slice(&x),
                                    Err(_) => {
                                        issues.unencodable += 1;
                                        b.extend_from_slice(if dbcs_only {
                                            &[0x81, 0x48]
                                        } else {
                                            b"?"
                                        });
                                    }
                                }
                            }
                            b
                        }
                    }
                };
                // 文字の境目で切る
                let mut i = 0;
                while i < enc.len() {
                    let n = if sjis_lead(enc[i]) && i + 1 < enc.len() {
                        2
                    } else {
                        1
                    };
                    if bytes.len() + n > len {
                        truncated = true;
                        break;
                    }
                    bytes.extend_from_slice(&enc[i..i + n]);
                    i += n;
                }
            }
            Charset::Ebcdic(c)
                if !dbcs_only && s.is_ascii() && {
                    let t = ascii_table(c);
                    s.bytes().all(|b| t[b as usize] != 0)
                } =>
            {
                // 速い道: ASCII だけの文字列
                let t = ascii_table(c);
                for b in s.bytes() {
                    if bytes.len() == len {
                        truncated = true;
                        break;
                    }
                    bytes.push(t[b as usize]);
                }
            }
            Charset::Ebcdic(c) => {
                let shift = c.shift_bytes();
                let mut in_dbcs = false;
                for ch in s.chars() {
                    // 元のバイト（エスケープ文字）・1 バイト・2 バイト
                    // （種類, バイト, バイト数）。種類は None が元のバイト、0 が 1 バイト、1 が 2 バイト
                    let unit: (Option<u8>, [u8; 2], usize) = if let Some(b) = unescape_char(ch) {
                        (None, [b, 0], 1)
                    } else {
                        let code = c.encode_char(ch).or_else(|| {
                            issues.unencodable += 1;
                            c.encode_char(if dbcs_only || ch.len_utf8() > 1 && shift.is_some() {
                                '？'
                            } else {
                                '?'
                            })
                        });
                        let code = match (code, dbcs_only) {
                            (Some(EbcdicCode::Single(_)), true) => {
                                match c.encode_char(to_fullwidth(ch)) {
                                    Some(EbcdicCode::Double(d)) => Some(EbcdicCode::Double(d)),
                                    _ => {
                                        issues.unencodable += 1;
                                        c.encode_char('？')
                                    }
                                }
                            }
                            (x, _) => x,
                        };
                        match code {
                            Some(EbcdicCode::Single(b)) => (Some(0), [b, 0], 1),
                            Some(EbcdicCode::Double(d)) => (Some(1), d.to_be_bytes(), 2),
                            None => (Some(0), [0x6F, 0], 1),
                        }
                    };
                    // 切り替え（SO / SI）が要るか
                    let (pre, post_dbcs) = match (unit.0, shift, dbcs_only) {
                        (Some(1), Some((so, _)), false) if !in_dbcs => (Some(so), true),
                        (Some(0), Some((_, si)), false) if in_dbcs => (Some(si), false),
                        (Some(1), _, _) => (None, !dbcs_only && shift.is_some()),
                        (Some(0), _, _) => (None, false),
                        _ => (None, in_dbcs),
                    };
                    let close = (post_dbcs && !dbcs_only) as usize;
                    if bytes.len() + pre.is_some() as usize + unit.2 + close > len {
                        truncated = true;
                        break;
                    }
                    if let Some(p) = pre {
                        bytes.push(p);
                    }
                    bytes.extend_from_slice(&unit.1[..unit.2]);
                    in_dbcs = post_dbcs;
                }
                if in_dbcs && let Some((_, si)) = shift {
                    bytes.push(si);
                }
            }
        }
        if truncated {
            issues.truncated += 1;
        }
        // 埋める
        let pad = len - bytes.len();
        let (text_at, pad_at) = if justified {
            (pad, 0)
        } else {
            (0, bytes.len())
        };
        out[text_at..text_at + bytes.len()].copy_from_slice(&bytes);
        let sp = self.charset.dbcs_space();
        for (i, o) in out[pad_at..pad_at + pad].iter_mut().enumerate() {
            *o = if dbcs_only {
                sp[i % 2]
            } else {
                self.charset.space()
            };
        }
    }

    // ---- 読む ----

    /// 項目のバイト列（`b.len() == f.len`）を読む。
    pub fn decode(&self, f: &Field, b: &[u8]) -> Decoded {
        let space = self.charset.space();
        match &f.kind {
            Kind::Alnum { justified } => {
                let s = self.decode_text(b, false);
                let t = s.trim_end_matches(' ');
                let t = if *justified {
                    t.trim_start_matches(' ')
                } else {
                    t
                };
                if t.is_empty() {
                    Decoded::Empty
                } else {
                    Decoded::Text(t.to_string())
                }
            }
            Kind::Dbcs => {
                let s = self.decode_text(b, true);
                let t = s.trim_end_matches(['\u{3000}', ' ']);
                if t.is_empty() {
                    Decoded::Empty
                } else {
                    Decoded::Text(t.to_string())
                }
            }
            Kind::National => {
                let units = b.chunks_exact(2).map(|p| u16::from_be_bytes([p[0], p[1]]));
                let s: String = char::decode_utf16(units)
                    .map(|r| r.unwrap_or('\u{FFFD}'))
                    .collect();
                let t = s.trim_end_matches([' ', '\u{3000}']);
                if t.is_empty() {
                    Decoded::Empty
                } else {
                    Decoded::Text(t.to_string())
                }
            }
            Kind::Zoned {
                scale,
                signed,
                sign,
                ..
            } => {
                if b.iter().all(|&x| x == space) {
                    return Decoded::Empty;
                }
                match self.decode_zoned(b, *signed, *sign) {
                    Some(v) => Decoded::Num(Decimal::new(v, *scale)),
                    None => Decoded::Invalid,
                }
            }
            Kind::Packed { scale, .. } => match decode_packed(b) {
                Some(v) => Decoded::Num(Decimal::new(v, *scale)),
                None => Decoded::Invalid,
            },
            Kind::Binary { scale, signed, .. } => {
                let mut buf = [0u8; 8];
                let n = b.len().min(8);
                if self.little_endian {
                    buf[..n].copy_from_slice(&b[..n]);
                } else {
                    for i in 0..n {
                        buf[i] = b[n - 1 - i];
                    }
                }
                let u = u64::from_le_bytes(buf);
                let v: i128 = if *signed {
                    // 符号を広げる
                    let shift = 64 - 8 * n as u32;
                    (((u << shift) as i64) >> shift) as i128
                } else {
                    u as i128
                };
                Decoded::Num(Decimal::new(v, *scale))
            }
            Kind::Float { double } => {
                let x = if self.charset.is_ebcdic() {
                    hfp_to_f64(b)
                } else if *double {
                    let a: [u8; 8] = b.try_into().unwrap_or([0; 8]);
                    if self.little_endian {
                        f64::from_le_bytes(a)
                    } else {
                        f64::from_be_bytes(a)
                    }
                } else {
                    let a: [u8; 4] = b.try_into().unwrap_or([0; 4]);
                    (if self.little_endian {
                        f32::from_le_bytes(a)
                    } else {
                        f32::from_be_bytes(a)
                    }) as f64
                };
                if x.is_finite() {
                    Decoded::Float(x)
                } else {
                    Decoded::Invalid
                }
            }
            Kind::Edited { syms, scale, .. } => {
                let s = self.decode_text(b, false);
                match edit::parse(syms, &s) {
                    Some(None) => Decoded::Empty,
                    Some(Some(d)) => Decoded::Num(Decimal::new(d.value, *scale)),
                    None => Decoded::Text(s.trim_end().to_string()),
                }
            }
        }
    }

    fn decode_zoned(&self, b: &[u8], signed: bool, sign: SignPos) -> Option<i128> {
        let ebcdic = self.charset.is_ebcdic();
        let (plus, minus) = self.charset.plus_minus();
        let (digits, sep): (&[u8], Option<u8>) = match (signed, sign) {
            (true, SignPos::LeadingSeparate) => (&b[1..], Some(b[0])),
            (true, SignPos::TrailingSeparate) => (&b[..b.len() - 1], Some(b[b.len() - 1])),
            _ => (b, None),
        };
        let mut neg = match sep {
            Some(x) if x == plus => false,
            Some(x) if x == minus => true,
            Some(_) => return None,
            None => false,
        };
        let sign_at = match (signed, sign, sep) {
            (_, _, Some(_)) => None,
            (true, SignPos::Leading, _) => Some(0),
            _ => Some(digits.len() - 1),
        };
        let mut v: i128 = 0;
        for (i, &x) in digits.iter().enumerate() {
            let d = if Some(i) == sign_at {
                let (d, n) = if ebcdic {
                    let d = x & 0x0F;
                    match x >> 4 {
                        0xC | 0xA | 0xE | 0xF => (d, false),
                        0xD | 0xB => (d, true),
                        _ => return None,
                    }
                } else {
                    match x {
                        b'0'..=b'9' => (x - b'0', false),
                        0x70..=0x79 => (x - 0x70, true),
                        b'{' => (0, false),
                        b'A'..=b'I' => (x - b'A' + 1, false),
                        b'}' => (0, true),
                        b'J'..=b'R' => (x - b'J' + 1, true),
                        _ => return None,
                    }
                };
                if d > 9 {
                    return None;
                }
                neg = n;
                d
            } else if ebcdic {
                if x >> 4 != 0xF || x & 0x0F > 9 {
                    return None;
                }
                x & 0x0F
            } else {
                if !x.is_ascii_digit() {
                    return None;
                }
                x - b'0'
            };
            v = v.checked_mul(10)?.checked_add(d as i128)?;
        }
        Some(if neg { -v } else { v })
    }

    // ---- 書く ----

    /// 値を項目のバイト列（`out.len() == f.len`）に書く。
    pub fn encode(&self, f: &Field, v: Input<'_>, out: &mut [u8], issues: &mut Issues) {
        let v = match v {
            Input::Error => {
                issues.not_number += f.kind.is_numeric() as u64;
                Input::Empty
            }
            v => v,
        };
        // 数値の項目に X'…' なら元のバイト
        if f.kind.is_numeric()
            && let Input::Text(s) = v
            && let Some(raw) = parse_hex(s, out.len())
        {
            out.copy_from_slice(&raw);
            return;
        }
        match &f.kind {
            Kind::Alnum { justified } => {
                let s = text_of(v);
                self.encode_text(&s, out, false, *justified, issues);
            }
            Kind::Dbcs => {
                let s = text_of(v);
                self.encode_text(&s, out, true, false, issues);
            }
            Kind::National => {
                let s = text_of(v);
                let mut units: Vec<u16> = Vec::new();
                for ch in s.chars() {
                    let mut buf = [0u16; 2];
                    let u = ch.encode_utf16(&mut buf);
                    if (units.len() + u.len()) * 2 > out.len() {
                        issues.truncated += 1;
                        break;
                    }
                    units.extend_from_slice(u);
                }
                while units.len() * 2 < out.len() {
                    units.push(0x0020);
                }
                for (i, u) in units.iter().enumerate() {
                    out[i * 2..i * 2 + 2].copy_from_slice(&u.to_be_bytes());
                }
            }
            Kind::Float { double } => {
                let x = match v {
                    Input::Number(x) => x,
                    Input::Bool(b) => b as u8 as f64,
                    Input::Text(s) => match Decimal::parse(s) {
                        Some(d) => d.to_f64(),
                        None => {
                            issues.not_number += 1;
                            0.0
                        }
                    },
                    _ => 0.0,
                };
                if self.charset.is_ebcdic() {
                    if !f64_to_hfp(x, out) {
                        issues.overflow += 1;
                        f64_to_hfp(0.0, out);
                    }
                } else if *double {
                    out.copy_from_slice(&if self.little_endian {
                        x.to_le_bytes()
                    } else {
                        x.to_be_bytes()
                    });
                } else {
                    let y = x as f32;
                    out.copy_from_slice(&if self.little_endian {
                        y.to_le_bytes()
                    } else {
                        y.to_be_bytes()
                    });
                }
            }
            Kind::Zoned {
                digits,
                scale,
                signed,
                sign,
            } => {
                let Some(d) = self.number(v, *scale, issues) else {
                    out.fill(self.charset.space());
                    return;
                };
                let (abs, neg) = self.fit(d.value, *digits, *signed, issues);
                self.encode_zoned(abs, neg, *digits, *signed, *sign, out);
            }
            Kind::Packed {
                digits,
                scale,
                signed,
            } => {
                let d = self
                    .number(v, *scale, issues)
                    .unwrap_or(Decimal::new(0, *scale));
                let (abs, neg) = self.fit(d.value, *digits, *signed, issues);
                encode_packed(abs, neg, *signed, out);
            }
            Kind::Binary {
                digits,
                scale,
                signed,
                bytes,
                native,
            } => {
                let d = self
                    .number(v, *scale, issues)
                    .unwrap_or(Decimal::new(0, *scale));
                let mut x = d.value;
                if !*signed && x < 0 {
                    issues.negative += 1;
                    x = -x;
                }
                let (lo, hi): (i128, i128) = match (*signed, *bytes) {
                    (true, n) => (-(1i128 << (8 * n - 1)), (1i128 << (8 * n - 1)) - 1),
                    (false, n) => (0, (1i128 << (8 * n)) - 1),
                };
                let limit = if *native { hi } else { 10i128.pow(*digits) - 1 };
                if x > limit || x < lo.max(-limit) {
                    issues.overflow += 1;
                    if *native {
                        x = x.rem_euclid(hi - lo + 1) + lo.min(0);
                    } else {
                        x %= 10i128.pow(*digits);
                    }
                }
                let le = (x as i64).to_le_bytes();
                let n = *bytes;
                for i in 0..n {
                    out[if self.little_endian { i } else { n - 1 - i }] = le[i];
                }
            }
            Kind::Edited {
                syms,
                scale,
                blank_zero,
                ..
            } => {
                if let Input::Text(s) = v
                    && Decimal::parse(s).is_none()
                    && !s.trim().is_empty()
                {
                    // 数値でない文字列はそのまま
                    self.encode_text(s, out, false, false, issues);
                    return;
                }
                let Some(d) = self.number(v, *scale, issues) else {
                    out.fill(self.charset.space());
                    return;
                };
                let (text, over) = edit::format(syms, d, *blank_zero);
                if over {
                    issues.overflow += 1;
                }
                self.encode_text(&text, out, false, false, issues);
            }
        }
    }

    /// 値を小数部 `scale` 桁の 10 進数に（空なら `None`）。
    fn number(&self, v: Input<'_>, scale: i32, issues: &mut Issues) -> Option<Decimal> {
        let d = match v {
            Input::Empty | Input::Error => return None,
            Input::Number(x) => Decimal::from_f64(x, scale),
            Input::Bool(b) => Decimal::new(b as i128, 0).rescale(scale),
            Input::Text(s) if s.trim().is_empty() => return None,
            Input::Text(s) => Decimal::parse(s).and_then(|d| d.rescale(scale)),
        };
        match d {
            Some(d) => Some(d),
            None => {
                issues.not_number += 1;
                Some(Decimal::new(0, scale))
            }
        }
    }

    /// 桁数に収める（あふれは上の桁を落とす。符号なしの負は絶対値）。
    fn fit(&self, v: i128, digits: u32, signed: bool, issues: &mut Issues) -> (u128, bool) {
        let mut neg = v < 0;
        if neg && !signed {
            issues.negative += 1;
            neg = false;
        }
        let mut abs = v.unsigned_abs();
        let max = 10u128.pow(digits.min(38));
        if abs >= max {
            issues.overflow += 1;
            abs %= max;
        }
        (abs, neg && abs != 0)
    }

    fn encode_zoned(
        &self,
        abs: u128,
        neg: bool,
        digits: u32,
        signed: bool,
        sign: SignPos,
        out: &mut [u8],
    ) {
        let ebcdic = self.charset.is_ebcdic();
        let mut buf = [0u8; 40];
        let ds = &mut buf[..digits as usize];
        digits_into(abs, ds);
        let digit = |d: u8| if ebcdic { 0xF0 | d } else { b'0' + d };
        let (plus, minus) = self.charset.plus_minus();
        let body: &mut [u8] = match (signed, sign) {
            (true, SignPos::LeadingSeparate) => {
                out[0] = if neg { minus } else { plus };
                &mut out[1..]
            }
            (true, SignPos::TrailingSeparate) => {
                let n = out.len();
                out[n - 1] = if neg { minus } else { plus };
                &mut out[..n - 1]
            }
            _ => out,
        };
        for (i, d) in ds.iter().enumerate() {
            body[i] = digit(*d);
        }
        if signed && matches!(sign, SignPos::Trailing | SignPos::Leading) {
            let at = if sign == SignPos::Leading {
                0
            } else {
                body.len() - 1
            };
            let d = ds[at];
            body[at] = match (ebcdic, neg) {
                (true, false) => 0xC0 | d,
                (true, true) => 0xD0 | d,
                (false, false) => b'0' + d,
                (false, true) => 0x70 + d,
            };
        }
    }
}

/// 絶対値の下から `n` 桁（上は 0 で埋める。`out` の長さが `n`）。
fn digits_into(mut v: u128, out: &mut [u8]) {
    for d in out.iter_mut().rev() {
        *d = (v % 10) as u8;
        v /= 10;
    }
}

fn text_of(v: Input<'_>) -> String {
    match v {
        Input::Empty | Input::Error => String::new(),
        Input::Text(s) => s.to_string(),
        Input::Bool(true) => "TRUE".into(),
        Input::Bool(false) => "FALSE".into(),
        Input::Number(x) => {
            if x.fract() == 0.0 && x.abs() < 1e15 {
                format!("{}", x as i64)
            } else {
                format!("{x}")
            }
        }
    }
}

fn decode_packed(b: &[u8]) -> Option<i128> {
    let n = b.len();
    let mut v: i128 = 0;
    for (i, &x) in b.iter().enumerate() {
        let hi = x >> 4;
        let lo = x & 0x0F;
        if hi > 9 {
            return None;
        }
        v = v.checked_mul(10)?.checked_add(hi as i128)?;
        if i + 1 < n {
            if lo > 9 {
                return None;
            }
            v = v.checked_mul(10)?.checked_add(lo as i128)?;
        } else {
            return match lo {
                0xC | 0xA | 0xE | 0xF => Some(v),
                0xD | 0xB => Some(-v),
                _ => None,
            };
        }
    }
    None
}

fn encode_packed(abs: u128, neg: bool, signed: bool, out: &mut [u8]) {
    let nibbles = out.len() * 2 - 1;
    let mut buf = [0u8; 80];
    let ds = &mut buf[..nibbles.min(80)];
    digits_into(abs, ds);
    let sign = match (signed, neg) {
        (false, _) => 0xF,
        (true, false) => 0xC,
        (true, true) => 0xD,
    };
    for (i, o) in out.iter_mut().enumerate() {
        let hi = ds[i * 2];
        let lo = if i * 2 + 1 < nibbles {
            ds[i * 2 + 1]
        } else {
            sign
        };
        *o = hi << 4 | lo;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;

    fn field(pic_line: &str) -> Field {
        parse(&format!("01 R.\n 05 F {pic_line}.\n"))
            .unwrap()
            .fields
            .remove(0)
    }

    fn enc(c: &Codec, f: &Field, v: Input<'_>) -> (Vec<u8>, Issues) {
        let mut out = vec![0u8; f.len];
        let mut is = Issues::default();
        c.encode(f, v, &mut out, &mut is);
        (out, is)
    }

    fn num(s: &str) -> Decimal {
        Decimal::parse(s).unwrap()
    }

    #[test]
    fn packed_zoned_binary_ebcdic() {
        let c = Codec::new(Charset::Ebcdic(Ccsid::Ibm930));
        let f = field("PIC S9(5) COMP-3");
        assert_eq!(enc(&c, &f, Input::Number(12345.0)).0, [0x12, 0x34, 0x5C]);
        assert_eq!(enc(&c, &f, Input::Number(-12345.0)).0, [0x12, 0x34, 0x5D]);
        assert_eq!(
            c.decode(&f, &[0x12, 0x34, 0x5D]),
            Decoded::Num(num("-12345"))
        );
        assert_eq!(
            c.decode(&f, &[0x12, 0x34, 0x5A]),
            Decoded::Num(num("12345"))
        );
        assert_eq!(c.decode(&f, &[0x40, 0x40, 0x40]), Decoded::Invalid);
        let f = field("PIC 9(4) COMP-3");
        assert_eq!(enc(&c, &f, Input::Text("12")).0, [0x00, 0x01, 0x2F]);
        let (b, is) = enc(&c, &f, Input::Number(123456.0));
        assert_eq!((b, is.overflow), (vec![0x03, 0x45, 0x6F], 1));
        let f = field("PIC S9(7)V99 COMP-3");
        assert_eq!(
            enc(&c, &f, Input::Number(-1234.5)).0,
            [0x00, 0x01, 0x23, 0x45, 0x0D]
        );
        assert_eq!(
            c.decode(&f, &[0x00, 0x01, 0x23, 0x45, 0x0D]),
            Decoded::Num(num("-1234.50"))
        );
        // ゾーン
        let f = field("PIC S9(3)");
        assert_eq!(enc(&c, &f, Input::Number(-123.0)).0, [0xF1, 0xF2, 0xD3]);
        assert_eq!(enc(&c, &f, Input::Number(123.0)).0, [0xF1, 0xF2, 0xC3]);
        assert_eq!(c.decode(&f, &[0xF1, 0xF2, 0xD3]), Decoded::Num(num("-123")));
        assert_eq!(c.decode(&f, &[0x40; 3]), Decoded::Empty);
        assert_eq!(c.decode(&f, &[0xF1, 0x40, 0xC3]), Decoded::Invalid);
        let f = field("PIC 9(3)");
        assert_eq!(enc(&c, &f, Input::Number(7.0)).0, [0xF0, 0xF0, 0xF7]);
        let (b, is) = enc(&c, &f, Input::Number(-7.0));
        assert_eq!((b, is.negative), (vec![0xF0, 0xF0, 0xF7], 1));
        let f = field("PIC S9(3) SIGN LEADING SEPARATE");
        assert_eq!(enc(&c, &f, Input::Number(-5.0)).0, [0x60, 0xF0, 0xF0, 0xF5]);
        assert_eq!(
            c.decode(&f, &[0x4E, 0xF1, 0xF2, 0xF3]),
            Decoded::Num(num("123"))
        );
        // 2 進数
        let f = field("PIC S9(4) COMP");
        assert_eq!(enc(&c, &f, Input::Number(-2.0)).0, [0xFF, 0xFE]);
        assert_eq!(c.decode(&f, &[0xFF, 0xFE]), Decoded::Num(num("-2")));
        let (_, is) = enc(&c, &f, Input::Number(12345.0));
        assert_eq!(is.overflow, 1);
        let f = field("PIC S9(4) COMP-5");
        let (b, is) = enc(&c, &f, Input::Number(12345.0));
        assert_eq!((b, is.overflow), (vec![0x30, 0x39], 0));
        let le = Codec {
            little_endian: true,
            ..c
        };
        let f = field("PIC 9(9) BINARY");
        assert_eq!(enc(&le, &f, Input::Number(300.0)).0, [0x2C, 0x01, 0, 0]);
        assert_eq!(le.decode(&f, &[0x2C, 0x01, 0, 0]), Decoded::Num(num("300")));
        // 浮動小数点（HFP）
        let f = field("COMP-1");
        assert_eq!(enc(&c, &f, Input::Number(1.0)).0, [0x41, 0x10, 0, 0]);
        assert_eq!(
            c.decode(&f, &[0xC2, 0x76, 0xA0, 0x00]),
            Decoded::Float(-118.625)
        );
        // 読めないバイトは X'…' で元に戻る
        let f = field("PIC S9(5) COMP-3");
        let hex = hex_text(&[0x40, 0x40, 0x40]);
        assert_eq!(hex, "X'404040'");
        assert_eq!(enc(&c, &f, Input::Text(&hex)).0, [0x40, 0x40, 0x40]);
    }

    #[test]
    fn zoned_ms932_and_ieee() {
        let c = Codec::new(Charset::Ms932);
        let f = field("PIC S9(3)V9");
        assert_eq!(enc(&c, &f, Input::Number(-12.3)).0, *b"012s");
        assert_eq!(c.decode(&f, b"012s"), Decoded::Num(num("-12.3")));
        assert_eq!(c.decode(&f, b"012L"), Decoded::Num(num("-12.3")));
        assert_eq!(c.decode(&f, b"012C"), Decoded::Num(num("12.3")));
        assert_eq!(c.decode(&f, b"    "), Decoded::Empty);
        assert_eq!(enc(&c, &f, Input::Empty).0, *b"    ");
        let f = field("COMP-2");
        assert_eq!(enc(&c, &f, Input::Number(1.5)).0, 1.5f64.to_be_bytes());
        assert_eq!(c.decode(&f, &1.5f64.to_be_bytes()), Decoded::Float(1.5));
        let f = field("PIC ZZ,ZZ9.99-");
        assert_eq!(enc(&c, &f, Input::Number(-1234.5)).0, *b" 1,234.50-");
        assert_eq!(c.decode(&f, b" 1,234.50-"), Decoded::Num(num("-1234.50")));
        assert_eq!(c.decode(&f, b"          "), Decoded::Empty);
    }

    #[test]
    fn text_fields() {
        let c = Codec::new(Charset::Ms932);
        let f = field("PIC X(6)");
        assert_eq!(enc(&c, &f, Input::Text("AB")).0, *b"AB    ");
        assert_eq!(
            enc(&c, &f, Input::Text("あいう")).0,
            [0x82, 0xA0, 0x82, 0xA2, 0x82, 0xA4]
        );
        // 文字の途中では切らない
        let (b, is) = enc(&c, &f, Input::Text("Aあいう"));
        assert_eq!(
            (b, is.truncated),
            (vec![b'A', 0x82, 0xA0, 0x82, 0xA2, b' '], 1)
        );
        assert_eq!(c.decode(&f, b"AB    "), Decoded::Text("AB".into()));
        assert_eq!(c.decode(&f, b"      "), Decoded::Empty);
        assert_eq!(
            c.decode(&f, &[0x82, 0xA0, b' ', b' ', b' ', b' ']),
            Decoded::Text("あ".into())
        );
        let f = field("PIC N(3)");
        assert_eq!(
            enc(&c, &f, Input::Text("A")).0,
            [0x82, 0x60, 0x81, 0x40, 0x81, 0x40]
        );
        let f = field("PIC X(4) JUSTIFIED RIGHT");
        assert_eq!(enc(&c, &f, Input::Text("12")).0, *b"  12");
        assert_eq!(c.decode(&f, b"  12"), Decoded::Text("12".into()));
        // EBCDIC の漢字（SO / SI）
        let e = Codec::new(Charset::Ebcdic(Ccsid::Ibm930));
        let f = field("PIC X(8)");
        let (b, _) = enc(&e, &f, Input::Text("A漢字"));
        assert_eq!(b[0], 0xC1);
        assert_eq!(b[1], 0x0E);
        assert_eq!(b[6], 0x0F);
        assert_eq!(b[7], 0x40);
        assert_eq!(e.decode(&f, &b), Decoded::Text("A漢字".into()));
        // SI が入らないなら 2 バイト文字を入れない
        let f6 = field("PIC X(5)");
        let (b, is) = enc(&e, &f6, Input::Text("A漢字"));
        assert_eq!(is.truncated, 1);
        assert_eq!(b[..5], [0xC1, 0x0E, b[2], b[3], 0x0F]);
        // PIC G（SO / SI なし）
        let g = field("PIC G(2)");
        let (b, _) = e_enc(&e, &g, "漢");
        assert_eq!(b[2..], [0x40, 0x40]);
        assert_eq!(e.decode(&g, &b), Decoded::Text("漢".into()));
        // 表にないバイトも元に戻る
        let x = field("PIC X(2)");
        let d = e.decode(&x, &[0xC1, 0xFF]);
        let Decoded::Text(t) = d else { panic!() };
        assert_eq!(enc(&e, &x, Input::Text(&t)).0, [0xC1, 0xFF]);
        // NATIONAL
        let n = field("PIC N(2) USAGE NATIONAL");
        assert_eq!(enc(&c, &n, Input::Text("あ")).0, [0x30, 0x42, 0x00, 0x20]);
        assert_eq!(
            c.decode(&n, &[0x30, 0x42, 0x00, 0x20]),
            Decoded::Text("あ".into())
        );
    }

    fn e_enc(c: &Codec, f: &Field, s: &str) -> (Vec<u8>, Issues) {
        enc(c, f, Input::Text(s))
    }

    #[test]
    fn charset_names() {
        assert_eq!(Charset::from_name("ms932"), Some(Charset::Ms932));
        assert_eq!(
            Charset::from_name("IBM-930"),
            Some(Charset::Ebcdic(Ccsid::Ibm930))
        );
        assert_eq!(
            Charset::from_name("1399"),
            Some(Charset::Ebcdic(Ccsid::Ibm1399))
        );
        assert_eq!(
            Charset::from_name("cp037"),
            Some(Charset::Ebcdic(Ccsid::Ibm037))
        );
        for c in Charset::all() {
            assert_eq!(Charset::from_name(c.name()), Some(c));
        }
    }
}
