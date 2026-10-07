//! `PIC`（`PICTURE`）文字列の解析と、`USAGE` と合わせた項目の型・長さ。

use crate::{Kind, SignPos, Sym};

/// `USAGE`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Usage {
    Display,
    Display1,
    National,
    /// `COMP`・`COMP-4`・`BINARY`
    Binary,
    /// `COMP-5`
    Comp5,
    Comp1,
    Comp2,
    Comp3,
    /// `POINTER`・`INDEX`（4 バイトの 2 進数として読む）
    Pointer,
}

impl Usage {
    /// `USAGE` の語から。
    pub fn from_word(w: &str) -> Option<Usage> {
        Some(match w {
            "DISPLAY" => Usage::Display,
            "DISPLAY-1" => Usage::Display1,
            "NATIONAL" => Usage::National,
            "COMP" | "COMP-4" | "COMPUTATIONAL" | "COMPUTATIONAL-4" | "BINARY" => Usage::Binary,
            "COMP-5" | "COMPUTATIONAL-5" => Usage::Comp5,
            "COMP-1" | "COMPUTATIONAL-1" => Usage::Comp1,
            "COMP-2" | "COMPUTATIONAL-2" => Usage::Comp2,
            "COMP-3" | "COMPUTATIONAL-3" | "PACKED-DECIMAL" => Usage::Comp3,
            "POINTER" | "INDEX" | "PROCEDURE-POINTER" | "FUNCTION-POINTER" => Usage::Pointer,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Usage::Display => "DISPLAY",
            Usage::Display1 => "DISPLAY-1",
            Usage::National => "NATIONAL",
            Usage::Binary => "COMP",
            Usage::Comp5 => "COMP-5",
            Usage::Comp1 => "COMP-1",
            Usage::Comp2 => "COMP-2",
            Usage::Comp3 => "COMP-3",
            Usage::Pointer => "POINTER",
        }
    }
}

/// `PIC` の記号の並び（繰り返し `X(10)` を展開したもの）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum P {
    A,
    X,
    S,
    N,
    G,
    E,
    Edit(Sym),
}

fn expand(pic: &str) -> Result<Vec<P>, String> {
    let chars: Vec<char> = pic.chars().collect();
    let mut out: Vec<P> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i].to_ascii_uppercase();
        let two: String = chars[i..chars.len().min(i + 2)]
            .iter()
            .map(|c| c.to_ascii_uppercase())
            .collect();
        let sym = match c {
            _ if two == "CR" => {
                i += 1;
                P::Edit(Sym::Cr)
            }
            _ if two == "DB" => {
                i += 1;
                P::Edit(Sym::Db)
            }
            'A' => P::A,
            'X' => P::X,
            'S' => P::S,
            'N' => P::N,
            'G' => P::G,
            'E' => P::E,
            '9' => P::Edit(Sym::Nine),
            'Z' => P::Edit(Sym::Z),
            '*' => P::Edit(Sym::Star),
            ',' => P::Edit(Sym::Comma),
            '.' => P::Edit(Sym::Dot),
            'V' => P::Edit(Sym::V),
            'P' => P::Edit(Sym::P),
            '+' => P::Edit(Sym::Plus),
            '-' => P::Edit(Sym::Minus),
            'B' => P::Edit(Sym::B),
            '0' => P::Edit(Sym::Zero),
            '/' => P::Edit(Sym::Slash),
            '$' | '\\' | '¥' | '￥' => P::Edit(Sym::Cur(chars[i])),
            '(' => {
                let end = chars[i..]
                    .iter()
                    .position(|&c| c == ')')
                    .ok_or_else(|| format!("PIC {pic} の ( が閉じていません"))?;
                let n: usize = chars[i + 1..i + end]
                    .iter()
                    .collect::<String>()
                    .trim()
                    .parse()
                    .map_err(|_| format!("PIC {pic} の繰り返しの数が読めません"))?;
                let last = *out
                    .last()
                    .ok_or_else(|| format!("PIC {pic} が ( で始まっています"))?;
                if n == 0 || n > 1_000_000 {
                    return Err(format!("PIC {pic} の繰り返しの数が範囲外です"));
                }
                for _ in 1..n {
                    out.push(last);
                }
                i += end + 1;
                continue;
            }
            _ => return Err(format!("PIC {pic} の記号 {} が読めません", chars[i])),
        };
        out.push(sym);
        i += 1;
    }
    if out.is_empty() {
        return Err("PIC が空です".into());
    }
    Ok(out)
}

/// 項目の属性（`PIC` 以外）。
#[derive(Clone, Copy, Debug, Default)]
pub struct Attrs {
    pub usage: Option<Usage>,
    pub sign: Option<SignPos>,
    pub justified: bool,
    pub blank_zero: bool,
}

/// 数字の桁数と小数部の桁数（`P` を含む。`P` は左端か右端にだけ置ける）。
fn digits_scale(syms: &[Sym]) -> (u32, i32) {
    let digits = syms.iter().filter(|s| **s == Sym::Nine).count() as u32;
    let ps = syms.iter().filter(|s| **s == Sym::P).count() as i32;
    let first9 = syms.iter().position(|s| *s == Sym::Nine);
    let firstp = syms.iter().position(|s| *s == Sym::P);
    let scale = match (firstp, first9) {
        // VPP99・PP99: 小数点の後に P の数だけ 0 が並ぶ
        (Some(p), Some(n)) if p < n => ps + digits as i32,
        (Some(_), None) => ps,
        // 99PPP: 値は 10^P 倍
        (Some(_), Some(_)) => -ps,
        (None, _) => match syms.iter().position(|s| matches!(s, Sym::V | Sym::Dot)) {
            Some(v) => syms[v + 1..].iter().filter(|s| **s == Sym::Nine).count() as i32,
            None => 0,
        },
    };
    (digits, scale)
}

/// `PIC` と属性から、項目の型と長さ。`pic` がない項目（`COMP-1`・`COMP-2`・`POINTER`）は `None`。
pub fn kind_of(pic: Option<&str>, a: &Attrs) -> Result<(Kind, usize), String> {
    let usage = a.usage.unwrap_or(Usage::Display);
    match usage {
        Usage::Comp1 => return Ok((Kind::Float { double: false }, 4)),
        Usage::Comp2 => return Ok((Kind::Float { double: true }, 8)),
        Usage::Pointer => {
            return Ok((
                Kind::Binary {
                    digits: 9,
                    scale: 0,
                    signed: false,
                    bytes: 4,
                    native: true,
                },
                4,
            ));
        }
        _ => {}
    }
    let pic = pic.ok_or("PIC がありません")?;
    let ps = expand(pic)?;
    let has = |p: P| ps.contains(&p);
    // 2 バイト文字
    if has(P::N) || has(P::G) {
        let n = ps.len();
        let kind = if usage == Usage::National {
            Kind::National
        } else {
            Kind::Dbcs
        };
        return Ok((kind, n * 2));
    }
    if has(P::E) {
        return Err(format!(
            "PIC {pic}: 浮動小数点の編集（E）には対応していません"
        ));
    }
    // 英数字（英数字編集を含む）
    if has(P::A) || has(P::X) {
        let n = ps.len();
        return Ok((
            Kind::Alnum {
                justified: a.justified,
            },
            n,
        ));
    }
    let signed = has(P::S);
    let syms: Vec<Sym> = ps
        .iter()
        .filter_map(|p| match p {
            P::Edit(s) => Some(*s),
            _ => None,
        })
        .collect();
    let numeric_only = syms
        .iter()
        .all(|s| matches!(s, Sym::Nine | Sym::V | Sym::P));
    let (digits, scale) = digits_scale(&syms);
    if numeric_only {
        if digits == 0 {
            return Err(format!("PIC {pic} に桁がありません"));
        }
        if digits > 38 {
            return Err(format!("PIC {pic} の桁数が多すぎます（38 桁まで）"));
        }
        return Ok(match usage {
            Usage::Display => {
                let sign = if signed {
                    a.sign.unwrap_or(SignPos::Trailing)
                } else {
                    SignPos::Trailing
                };
                let sep =
                    signed && matches!(sign, SignPos::LeadingSeparate | SignPos::TrailingSeparate);
                (
                    Kind::Zoned {
                        digits,
                        scale,
                        signed,
                        sign,
                    },
                    digits as usize + sep as usize,
                )
            }
            Usage::Comp3 => (
                Kind::Packed {
                    digits,
                    scale,
                    signed,
                },
                digits as usize / 2 + 1,
            ),
            Usage::Binary | Usage::Comp5 => {
                let bytes = match digits {
                    1..=4 => 2,
                    5..=9 => 4,
                    10..=18 => 8,
                    _ => return Err(format!("PIC {pic}: 2 進数は 18 桁までです")),
                };
                (
                    Kind::Binary {
                        digits,
                        scale,
                        signed,
                        bytes,
                        native: usage == Usage::Comp5,
                    },
                    bytes,
                )
            }
            Usage::National | Usage::Display1 => {
                return Err(format!(
                    "PIC {pic}: {} の数字項目には対応していません",
                    usage.name()
                ));
            }
            _ => unreachable!(),
        });
    }
    // 数字編集
    if usage != Usage::Display {
        return Err(format!("PIC {pic}: 数字編集は DISPLAY だけです"));
    }
    let len = syms
        .iter()
        .map(|s| match s {
            Sym::V | Sym::P => 0,
            Sym::Cr | Sym::Db => 2,
            _ => 1,
        })
        .sum();
    let digit_pos = crate::edit::digit_positions(&syms);
    let digits = digit_pos.len() as u32;
    let scale = digit_pos.iter().filter(|p| !p.1).count() as i32;
    Ok((
        Kind::Edited {
            syms,
            digits,
            scale,
            blank_zero: a.blank_zero,
        },
        len,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(pic: &str, usage: Option<Usage>) -> (Kind, usize) {
        kind_of(
            Some(pic),
            &Attrs {
                usage,
                ..Default::default()
            },
        )
        .unwrap()
    }

    #[test]
    fn kinds_and_lengths() {
        assert_eq!(k("X(10)", None).1, 10);
        assert_eq!(k("XXBXX", None).1, 5);
        assert_eq!(k("N(5)", None), (Kind::Dbcs, 10));
        assert_eq!(k("N(5)", Some(Usage::National)), (Kind::National, 10));
        assert_eq!(
            k("S9(5)V99", None),
            (
                Kind::Zoned {
                    digits: 7,
                    scale: 2,
                    signed: true,
                    sign: SignPos::Trailing
                },
                7
            )
        );
        assert_eq!(k("S9(7)V99", Some(Usage::Comp3)).1, 5);
        assert_eq!(k("9(4)", Some(Usage::Comp3)).1, 3);
        assert_eq!(k("S9(4)", Some(Usage::Binary)).1, 2);
        assert_eq!(k("S9(9)", Some(Usage::Binary)).1, 4);
        assert_eq!(k("S9(18)", Some(Usage::Comp5)).1, 8);
        assert_eq!(k("99PPP", None).0.digits_scale(), Some((2, -3)));
        assert_eq!(k("VPP99", None).0.digits_scale(), Some((2, 4)));
        assert_eq!(k("ZZ,ZZ9.99-", None).1, 10);
        assert_eq!(k("ZZ,ZZ9.99-", None).0.digits_scale(), Some((7, 2)));
        assert_eq!(k("$$$,$$9.99CR", None).1, 12);
        assert_eq!(k("$$$,$$9.99CR", None).0.digits_scale(), Some((7, 2)));
        assert!(kind_of(None, &Attrs::default()).is_err());
        assert!(kind_of(Some("9(5)Q"), &Attrs::default()).is_err());
    }
}
