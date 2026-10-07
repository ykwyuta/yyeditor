//! 10 進数（桁を落とさない値）と、IBM の 16 進浮動小数点（HFP）。

/// 10 進数（`value × 10^-scale`）。38 桁まで。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Decimal {
    pub value: i128,
    pub scale: i32,
}

fn pow10(n: u32) -> Option<i128> {
    10i128.checked_pow(n)
}

impl Decimal {
    pub fn new(value: i128, scale: i32) -> Decimal {
        Decimal { value, scale }
    }

    /// 絶対値の桁数（0 は 0 桁）。
    pub fn digits(&self) -> u32 {
        let mut v = self.value.unsigned_abs();
        let mut n = 0;
        while v > 0 {
            v /= 10;
            n += 1;
        }
        n
    }

    pub fn to_f64(&self) -> f64 {
        format!("{}e{}", self.value, -self.scale)
            .parse()
            .unwrap_or(f64::NAN)
    }

    /// 小数部の桁数を `scale` にする（減らすときは四捨五入）。桁あふれなら `None`。
    pub fn rescale(self, scale: i32) -> Option<Decimal> {
        let d = scale - self.scale;
        if d >= 0 {
            let v = self.value.checked_mul(pow10(d as u32)?)?;
            return Some(Decimal::new(v, scale));
        }
        let p = pow10((-d) as u32)?;
        let q = self.value / p;
        let r = self.value % p;
        // 半分以上は絶対値を大きく（四捨五入）
        let v = if r.unsigned_abs() * 2 >= p.unsigned_abs() {
            q + self.value.signum()
        } else {
            q
        };
        Some(Decimal::new(v, scale))
    }

    /// 浮動小数点数から（小数部 `scale` 桁に四捨五入。値は最短の 10 進表記で見る: `1.005` → `1.01`）。
    pub fn from_f64(x: f64, scale: i32) -> Option<Decimal> {
        if !x.is_finite() || scale > 38 {
            return None;
        }
        // 速い道: 小数部の桁数を掛けた値が整数のごく近く（四捨五入の境目から遠い）なら、その整数
        if (0..=15).contains(&scale) {
            let y = x * 10f64.powi(scale);
            let r = y.round();
            if r.abs() < 9.0e15 && (y - r).abs() < 1e-6 {
                return Some(Decimal::new(r as i128, scale));
            }
        }
        Decimal::parse(&format!("{x}"))?.rescale(scale)
    }

    /// 文字列から（`1,234.5`・`-12`・`12-`・`+3`・全角の数字。前後の空白は無視）。
    pub fn parse(s: &str) -> Option<Decimal> {
        let t: String = s
            .trim()
            .chars()
            .map(|c| match c {
                '０'..='９' => char::from_u32(c as u32 - 0xFF10 + 0x30).unwrap_or(c),
                '－' | '−' => '-',
                '＋' => '+',
                '．' => '.',
                '，' => ',',
                _ => c,
            })
            .collect();
        let t = t.trim();
        if t.is_empty() {
            return None;
        }
        let (neg, body) = if let Some(r) = t.strip_prefix('-') {
            (true, r)
        } else if let Some(r) = t.strip_prefix('+') {
            (false, r)
        } else if let Some(r) = t.strip_suffix('-') {
            (true, r)
        } else if let Some(r) = t.strip_suffix('+') {
            (false, r)
        } else {
            (false, t)
        };
        let body = body.trim();
        if body.is_empty() {
            return None;
        }
        let mut v: i128 = 0;
        let mut scale = 0i32;
        let mut point = false;
        let mut any = false;
        for c in body.chars() {
            match c {
                '0'..='9' => {
                    v = v.checked_mul(10)?.checked_add((c as u8 - b'0') as i128)?;
                    any = true;
                    if point {
                        scale += 1;
                    }
                }
                '.' if !point => point = true,
                ',' if !point => {}
                _ => return None,
            }
        }
        if !any {
            return None;
        }
        Some(Decimal::new(if neg { -v } else { v }, scale))
    }
}

impl std::fmt::Display for Decimal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let neg = self.value < 0;
        let mut digits = self.value.unsigned_abs().to_string();
        if self.scale < 0 {
            digits.extend(std::iter::repeat_n('0', (-self.scale) as usize));
        }
        let scale = self.scale.max(0) as usize;
        if digits.len() <= scale {
            digits = "0".repeat(scale + 1 - digits.len()) + &digits;
        }
        if neg {
            f.write_str("-")?;
        }
        let (i, fr) = digits.split_at(digits.len() - scale);
        f.write_str(i)?;
        if scale > 0 {
            f.write_str(".")?;
            f.write_str(fr)?;
        }
        Ok(())
    }
}

/// IBM の 16 進浮動小数点（4 バイトか 8 バイト、ビッグエンディアン）を読む。
pub fn hfp_to_f64(b: &[u8]) -> f64 {
    let (frac, bits, exp, neg) = if b.len() == 4 {
        let v = u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
        (
            (v & 0x00FF_FFFF) as f64,
            24,
            ((v >> 24) & 0x7F) as i32,
            v >> 31 == 1,
        )
    } else {
        let v = u64::from_be_bytes(b[..8].try_into().unwrap_or([0; 8]));
        (
            (v & 0x00FF_FFFF_FFFF_FFFF) as f64,
            56,
            ((v >> 56) & 0x7F) as i32,
            v >> 63 == 1,
        )
    };
    let x = frac / 2f64.powi(bits) * 16f64.powi(exp - 64);
    if neg { -x } else { x }
}

/// 16 進浮動小数点に書く（`out` は 4 バイトか 8 バイト）。表せなければ `false`。
pub fn f64_to_hfp(x: f64, out: &mut [u8]) -> bool {
    out.fill(0);
    if x == 0.0 {
        return true;
    }
    if !x.is_finite() {
        return false;
    }
    let neg = x < 0.0;
    let mut m = x.abs();
    let mut exp = 64i32;
    while m >= 1.0 {
        m /= 16.0;
        exp += 1;
    }
    while m < 1.0 / 16.0 {
        m *= 16.0;
        exp -= 1;
    }
    let bits = if out.len() == 4 { 24 } else { 56 };
    let mut frac = (m * 2f64.powi(bits)).round() as u64;
    if frac >= 1u64 << bits {
        frac >>= 4;
        exp += 1;
    }
    if !(0..=127).contains(&exp) {
        return false;
    }
    let head = ((neg as u64) << 7 | exp as u64) << bits;
    let v = head | frac;
    if out.len() == 4 {
        out.copy_from_slice(&(v as u32).to_be_bytes());
    } else {
        out.copy_from_slice(&v.to_be_bytes());
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimal_parse_format_rescale() {
        let d = |s| Decimal::parse(s).unwrap();
        assert_eq!(d("1,234.50"), Decimal::new(123450, 2));
        assert_eq!(d(" -12 "), Decimal::new(-12, 0));
        assert_eq!(d("12-"), Decimal::new(-12, 0));
        assert_eq!(d("＋３．５"), Decimal::new(35, 1));
        assert!(Decimal::parse("12a").is_none());
        assert!(Decimal::parse("-").is_none());
        assert_eq!(Decimal::new(123450, 2).to_string(), "1234.50");
        assert_eq!(Decimal::new(-5, 3).to_string(), "-0.005");
        assert_eq!(Decimal::new(12, -3).to_string(), "12000");
        assert_eq!(d("1.235").rescale(2), Some(Decimal::new(124, 2)));
        assert_eq!(d("-1.235").rescale(2), Some(Decimal::new(-124, 2)));
        assert_eq!(d("1.5").rescale(3), Some(Decimal::new(1500, 3)));
        assert_eq!(Decimal::from_f64(0.1 + 0.2, 2), Some(Decimal::new(30, 2)));
        assert_eq!(Decimal::from_f64(-1234.5, 0), Some(Decimal::new(-1235, 0)));
        assert_eq!(Decimal::from_f64(12345.0, -3), Some(Decimal::new(12, -3)));
        assert_eq!(Decimal::new(-123450, 2).to_f64(), -1234.5);
        assert_eq!(Decimal::new(99999, 0).digits(), 5);
        // 31 桁も落とさない
        let big = d("1234567890123456789012345678.901");
        assert_eq!(big.to_string(), "1234567890123456789012345678.901");
    }

    #[test]
    fn hex_float() {
        // 1.0 = 41 10 00 00、-118.625 = C2 76 A0 00
        let mut b = [0u8; 4];
        assert!(f64_to_hfp(1.0, &mut b));
        assert_eq!(b, [0x41, 0x10, 0, 0]);
        assert!(f64_to_hfp(-118.625, &mut b));
        assert_eq!(b, [0xC2, 0x76, 0xA0, 0x00]);
        assert_eq!(hfp_to_f64(&[0xC2, 0x76, 0xA0, 0x00]), -118.625);
        let mut b8 = [0u8; 8];
        for x in [0.1, std::f64::consts::PI, -1e10, 1e-20] {
            assert!(f64_to_hfp(x, &mut b8));
            let y = hfp_to_f64(&b8);
            assert!(((y - x) / x).abs() < 1e-15, "{x} {y}");
        }
    }
}
