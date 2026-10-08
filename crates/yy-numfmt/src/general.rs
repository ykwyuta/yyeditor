//! 「標準」（General）の文字列（数式の連結・CSV への書き出し・セルの編集で使う、列幅によらない形）。
//!
//! Excel と同じく有効数字 15 桁に丸め、末尾の 0 を除く。整数部が 15 桁を超える数と、ごく小さい数は
//! 指数表記（`1.23456789012346E+17`・`1E-11`）にする。

/// 「標準」の文字列を `out` に足す（速い道: 15 桁までの整数と、小数 6 桁までで正確に表せる値）。
pub fn general_into(out: &mut Vec<u8>, v: f64) {
    let a = v.abs();
    if v.is_finite() && a < 1e15 {
        for (k, &scale) in SCALE.iter().enumerate() {
            let m = (a * scale).round();
            if m < 9e15 && m / scale == a {
                if v < 0.0 && m != 0.0 {
                    out.push(b'-');
                }
                let m = m as u64;
                let p = 10u64.pow(k as u32);
                push_u64(out, m / p);
                if k > 0 {
                    let mut f = m % p;
                    if f != 0 {
                        out.push(b'.');
                        let mut digits = [b'0'; 6];
                        for d in (0..k).rev() {
                            digits[d] = b'0' + (f % 10) as u8;
                            f /= 10;
                        }
                        let mut n = k;
                        while n > 0 && digits[n - 1] == b'0' {
                            n -= 1;
                        }
                        out.extend_from_slice(&digits[..n]);
                    }
                }
                return;
            }
        }
    }
    out.extend_from_slice(general(v).as_bytes());
}

const SCALE: [f64; 7] = [1.0, 10.0, 100.0, 1e3, 1e4, 1e5, 1e6];

fn push_u64(out: &mut Vec<u8>, mut n: u64) {
    let mut buf = [0u8; 20];
    let mut i = buf.len();
    loop {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    out.extend_from_slice(&buf[i..]);
}

/// 「標準」の文字列。
pub fn general(v: f64) -> String {
    if v.is_nan() {
        return "#NUM!".into();
    }
    if v.is_infinite() {
        return "#NUM!".into();
    }
    if v == 0.0 {
        return "0".into();
    }
    // 速い道: 15 桁までの整数、15 桁以内で正確に書ける小数
    let a = v.abs();
    if a < 1e15 {
        if v.fract() == 0.0 {
            return (v as i64).to_string();
        }
        if a >= 1e-9 {
            let s = v.to_string();
            let sig = s
                .bytes()
                .filter(u8::is_ascii_digit)
                .skip_while(|&c| c == b'0')
                .count();
            if sig <= 15 {
                return s;
            }
        }
    }
    let (neg, digits, exp) = round15(v);
    let mut s = String::new();
    if neg {
        s.push('-');
    }
    if (-10..15).contains(&exp) {
        fixed(&mut s, &digits, exp);
    } else {
        scientific(&mut s, &digits, exp);
    }
    s
}

/// 有効数字 15 桁に丸めた数字（末尾の 0 を除く）と、10 の指数（`d.ddd × 10^exp`）。
pub(crate) fn round15(v: f64) -> (bool, Vec<u8>, i32) {
    let t = format!("{:.14e}", v.abs());
    let (mant, exp) = t.split_once('e').expect("e");
    let exp: i32 = exp.parse().expect("exp");
    let mut digits: Vec<u8> = mant.bytes().filter(u8::is_ascii_digit).collect();
    while digits.len() > 1 && digits.last() == Some(&b'0') {
        digits.pop();
    }
    (v < 0.0, digits, exp)
}

fn fixed(s: &mut String, digits: &[u8], exp: i32) {
    if exp < 0 {
        s.push_str("0.");
        for _ in 0..(-exp - 1) {
            s.push('0');
        }
        s.extend(digits.iter().map(|&d| d as char));
        return;
    }
    let int_len = exp as usize + 1;
    for i in 0..int_len {
        s.push(digits.get(i).map_or('0', |&d| d as char));
    }
    if digits.len() > int_len {
        s.push('.');
        s.extend(digits[int_len..].iter().map(|&d| d as char));
    }
}

fn scientific(s: &mut String, digits: &[u8], exp: i32) {
    s.push(digits[0] as char);
    if digits.len() > 1 {
        s.push('.');
        s.extend(digits[1..].iter().map(|&d| d as char));
    }
    s.push('E');
    s.push(if exp < 0 { '-' } else { '+' });
    s.push_str(&format!("{:02}", exp.abs()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_excel_text_conversion() {
        for (v, t) in [
            (0.0, "0"),
            (-0.0, "0"),
            (1.0, "1"),
            (-1.5, "-1.5"),
            (0.1 + 0.2, "0.3"),
            (1234567.0, "1234567"),
            (123456789012345.0, "123456789012345"),
            (1234567890123456.0, "1.23456789012346E+15"),
            (1e20, "1E+20"),
            (0.001, "0.001"),
            (0.000000001, "0.000000001"),
            (1e-11, "1E-11"),
            (2.0 / 3.0, "0.666666666666667"),
            (100.0, "100"),
            (46302.75, "46302.75"),
        ] {
            assert_eq!(general(v), t, "{v}");
            let mut b = Vec::new();
            general_into(&mut b, v);
            assert_eq!(String::from_utf8(b).unwrap(), t, "{v}");
        }
    }
}
