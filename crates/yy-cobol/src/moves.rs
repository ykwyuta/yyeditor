//! COBOL の `MOVE`（yysheet の `CBL.MOVE`）。
//!
//! - **基本項目の MOVE**（送り出しと受け取りの項目が 1 つずつ）: 受け取り側の型に合わせて変える。
//!   - 数値 → 数値: 小数点の位置を揃え、小数部の多い桁を切り捨て、整数部のあふれは上の桁を落とす
//!     （`ON SIZE ERROR` はない）。符号なしの受け取りには絶対値。
//!   - 数値（整数）→ 英数字: 数値の項目の桁数の数字（上の 0 を含む。符号は付けない）を左詰めで。
//!     小数部のある数値の項目・浮動小数点は英数字に送れない（`None`）。型のない数値は桁のとおり。
//!   - 数字編集 → 英数字: 編集した文字列。数字編集 → 数値: 編集を外した値。
//!   - 英数字 → 数値: 数字だけの文字列を符号なしの整数として送る（数字でなければ `None`）。
//!   - 英数字 → 英数字: 左詰め（`JUSTIFIED` なら右詰め）、足りない分は空白、長ければ切る。
//! - **集団の MOVE**（項目の数が違う）: 送り出しの項目を並べたバイト列を、受け取りの項目を並べた
//!   領域へ英数字として左詰めで送る（足りない分は空白、長ければ切る）。項目の型は変えない（バイトの
//!   まま）ので、数値の項目の中身が数値として読めなくなることもある（COBOL と同じ）。
//!
//! 送り出しの値は受け取り側の文字コードで書く（COBOL のプログラムの中では文字コードは 1 つ）。

use crate::codec::{Codec, Decoded, Input, Issues};
use crate::num::Decimal;
use crate::{Field, Kind};

/// MOVE の結果（読んだ値と、受け取りの項目のバイト列）。
pub type Moved = (Decoded, Vec<u8>);

/// 送り出しの値の種類。
enum Cat {
    Numeric,
    Edited,
    Float,
    Alpha,
}

fn category(v: &Input<'_>, src: Option<&Field>) -> Cat {
    match src.map(|f| &f.kind) {
        Some(Kind::Edited { .. }) => Cat::Edited,
        Some(Kind::Float { .. }) => Cat::Float,
        Some(k) if k.is_numeric() => Cat::Numeric,
        Some(_) => Cat::Alpha,
        None => match v {
            Input::Number(_) => Cat::Numeric,
            _ => Cat::Alpha,
        },
    }
}

fn plain_text(v: &Input<'_>) -> String {
    match v {
        Input::Text(s) => s.to_string(),
        Input::Number(x) => {
            if x.fract() == 0.0 && x.abs() < 1e15 {
                format!("{}", *x as i64)
            } else {
                format!("{x}")
            }
        }
        Input::Bool(b) => (if *b { "TRUE" } else { "FALSE" }).into(),
        _ => String::new(),
    }
}

impl Codec {
    /// 項目のバイト列を書いて読み直す。
    fn put(&self, f: &Field, v: Input<'_>) -> Moved {
        let mut b = vec![0u8; f.len];
        self.encode(f, v, &mut b, &mut Issues::default());
        (self.decode(f, &b), b)
    }

    /// 基本項目の MOVE（`src` は送り出しの項目の型。型のない列なら `None`）。COBOL で送れない組み合わせ
    /// （小数部のある数値 → 英数字、数字でない文字列 → 数値など）は `None`。
    pub fn move_elementary(&self, v: Input<'_>, src: Option<&Field>, dst: &Field) -> Option<Moved> {
        match v {
            Input::Error => return None,
            Input::Fill(_) | Input::Empty => return Some(self.put(dst, v)),
            _ => {}
        }
        let to_num = dst.kind.is_numeric();
        match (category(&v, src), to_num) {
            (Cat::Numeric | Cat::Edited | Cat::Float, true) => Some(self.put(dst, v)),
            (Cat::Float, false) => None,
            (Cat::Numeric, false) => {
                if matches!(dst.kind, Kind::Dbcs) {
                    return None;
                }
                let digits = match src {
                    Some(f) => {
                        let (digits, scale) = f.kind.digits_scale()?;
                        if scale > 0 {
                            return None;
                        }
                        let d = match v {
                            Input::Number(x) => Decimal::from_f64(x, scale)?,
                            Input::Text(s) => Decimal::parse(s).and_then(|d| d.truncate(scale))?,
                            _ => return None,
                        };
                        // P の桁（99PPP）は 0 として並べる
                        let width = (digits as i32 - scale) as usize;
                        let abs =
                            d.value.unsigned_abs().to_string() + &"0".repeat((-scale) as usize);
                        format!("{abs:0>width$}")
                    }
                    None => match v {
                        Input::Number(x) if x.fract() == 0.0 && x.abs() < 1e18 => {
                            (x.abs() as u64).to_string()
                        }
                        _ => return None,
                    },
                };
                Some(self.put(dst, Input::Text(&digits)))
            }
            (Cat::Edited, false) => {
                // 編集した文字列（送り出しの項目に書いた文字）
                let f = src?;
                let mut b = vec![0u8; f.len];
                self.encode(f, v, &mut b, &mut Issues::default());
                let text_field = Field {
                    kind: Kind::Alnum {
                        justified: false,
                        alpha: false,
                    },
                    ..f.clone()
                };
                let s = match self.decode(&text_field, &b) {
                    Decoded::Text(s) => s,
                    _ => String::new(),
                };
                Some(self.put(dst, Input::Text(&s)))
            }
            (Cat::Alpha, true) => {
                let s = plain_text(&v);
                if s.is_empty() || !s.bytes().all(|c| c.is_ascii_digit()) {
                    return None;
                }
                Some(self.put(dst, Input::Text(&s)))
            }
            (Cat::Alpha, false) => {
                let s = plain_text(&v);
                Some(self.put(dst, Input::Text(&s)))
            }
        }
    }

    /// 集団の MOVE: 送り出しの項目（型のない列は値の文字列）を並べたバイト列を、受け取りの項目を
    /// 並べた領域へ左詰めで送る。
    pub fn move_group(&self, src: &[(Input<'_>, Option<&Field>)], dst: &[&Field]) -> Vec<Moved> {
        let mut bytes = Vec::new();
        let mut issues = Issues::default();
        for (v, f) in src {
            match f {
                Some(f) => {
                    let mut b = vec![0u8; f.len];
                    self.encode(f, *v, &mut b, &mut issues);
                    bytes.extend_from_slice(&b);
                }
                None => {
                    let s = plain_text(v);
                    bytes.extend(self.string_bytes(&s));
                }
            }
        }
        let total: usize = dst.iter().map(|f| f.len).sum();
        let mut area = vec![self.charset.space(); total];
        let n = bytes.len().min(total);
        area[..n].copy_from_slice(&bytes[..n]);
        let mut at = 0;
        dst.iter()
            .map(|f| {
                let b = area[at..at + f.len].to_vec();
                at += f.len;
                (self.decode(f, &b), b)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Charset, parse};

    fn field(pic: &str) -> Field {
        parse(&format!("01 R.\n 05 F {pic}.\n"))
            .unwrap()
            .fields
            .remove(0)
    }

    fn num(v: i128, s: i32) -> Decoded {
        Decoded::Num(Decimal::new(v, s))
    }

    #[test]
    fn elementary_moves() {
        let c = Codec::new(Charset::Ms932);
        let mv =
            |v: Input<'_>, s: Option<&Field>, d: &Field| c.move_elementary(v, s, d).map(|m| m.0);
        let n52 = field("PIC S9(5)V99");
        let n3 = field("PIC 9(3)");
        let x8 = field("PIC X(8)");
        // 数値 → 数値: 小数点を揃え、多い桁は切り捨て、上の桁は落とす、符号なしは絶対値
        assert_eq!(
            mv(Input::Number(12345.678), Some(&n52), &n3),
            Some(num(345, 0))
        );
        assert_eq!(mv(Input::Number(-7.5), Some(&n52), &n3), Some(num(7, 0)));
        assert_eq!(mv(Input::Number(-7.5), None, &n52), Some(num(-750, 2)));
        // 数値（整数）→ 英数字: 項目の桁数の数字
        assert_eq!(
            mv(Input::Number(42.0), Some(&field("PIC 9(5)")), &x8),
            Some(Decoded::Text("00042".into()))
        );
        assert_eq!(
            mv(Input::Number(-42.0), Some(&field("PIC S9(3)")), &x8),
            Some(Decoded::Text("042".into()))
        );
        assert_eq!(
            mv(Input::Number(42.0), None, &x8),
            Some(Decoded::Text("42".into()))
        );
        assert_eq!(mv(Input::Number(1.5), Some(&n52), &x8), None);
        assert_eq!(mv(Input::Number(1.5), None, &x8), None);
        assert_eq!(mv(Input::Number(1.0), Some(&field("COMP-2")), &x8), None);
        assert_eq!(
            mv(
                Input::Number(123456789.0),
                Some(&field("PIC 9(9)")),
                &field("PIC X(4)")
            ),
            Some(Decoded::Text("1234".into()))
        );
        // 数字編集 → 英数字は編集した文字列、数値へは値
        let ed = field("PIC ZZ,ZZ9.99-");
        assert_eq!(
            mv(Input::Number(-1234.5), Some(&ed), &field("PIC X(12)")),
            Some(Decoded::Text(" 1,234.50-".into()))
        );
        assert_eq!(
            mv(Input::Number(-1234.5), Some(&ed), &n52),
            Some(num(-123450, 2))
        );
        // 英数字 → 数値: 数字だけを整数として
        assert_eq!(
            mv(Input::Text("00123"), Some(&x8), &n52),
            Some(num(12300, 2))
        );
        assert_eq!(mv(Input::Text("12A"), Some(&x8), &n52), None);
        assert_eq!(mv(Input::Text("1.5"), None, &n52), None);
        // 英数字 → 英数字: 左詰め・切る・右詰め
        assert_eq!(
            mv(Input::Text("ABCDEFGHIJ"), None, &x8),
            Some(Decoded::Text("ABCDEFGH".into()))
        );
        assert_eq!(
            mv(Input::Text("AB"), None, &field("PIC X(5) JUSTIFIED RIGHT")),
            Some(Decoded::Text("AB".into()))
        );
        assert_eq!(
            c.move_elementary(Input::Text("AB"), None, &field("PIC X(5) JUSTIFIED RIGHT"))
                .unwrap()
                .1,
            b"   AB"
        );
        // 空・表意定数・エラー
        assert_eq!(
            c.move_elementary(Input::Fill(0), None, &n3).unwrap().1,
            vec![0, 0, 0]
        );
        assert_eq!(mv(Input::Error, None, &n3), None);
        assert_eq!(mv(Input::Empty, None, &x8), Some(Decoded::Empty));
    }

    #[test]
    fn group_moves() {
        let c = Codec::new(Charset::Ms932);
        let id = field("PIC 9(3)");
        let name = field("PIC X(4)");
        let amt = field("PIC S9(3)V99 COMP-3");
        let wide = field("PIC X(10)");
        // 3 項目 → 1 項目: バイトを並べて左詰め（パック 10 進数の 3 バイトもそのまま、空白で埋める）
        let r = c.move_group(
            &[
                (Input::Number(7.0), Some(&id)),
                (Input::Text("AB"), Some(&name)),
                (Input::Number(-1.5), Some(&amt)),
            ],
            &[&wide],
        );
        assert_eq!(r[0].1, b"007AB  \x00\x15\x0d".to_vec());
        // 1 項目 → 2 項目: 先頭から分ける（足りない分は空白）
        let r = c.move_group(&[(Input::Text("12345XY"), Some(&wide))], &[&id, &name]);
        assert_eq!(r[0].0, num(123, 0));
        assert_eq!(r[1].0, Decoded::Text("45XY".into()));
        // 数値の項目に数字でないバイトが入れば読めない（COBOL と同じ）
        let r = c.move_group(&[(Input::Text("AB"), None)], &[&id, &name]);
        assert_eq!(r[0].0, Decoded::Invalid);
        assert_eq!(r[1].0, Decoded::Empty);
        // 長ければ切る
        let r = c.move_group(
            &[
                (Input::Text("ABCDEFGHIJ"), Some(&wide)),
                (Input::Text("KL"), None),
            ],
            &[&name, &id],
        );
        assert_eq!(r[0].1, b"ABCD");
        assert_eq!(r[1].0, Decoded::Invalid);
    }
}
