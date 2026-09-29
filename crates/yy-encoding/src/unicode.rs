//! UTF-16 / UTF-32。
//!
//! 不対サロゲートや範囲外の値など、文字にならない符号単位はそのバイト列をエスケープ文字にする。

use crate::Sink;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Form {
    Utf16Le,
    Utf16Be,
    Utf32Le,
    Utf32Be,
}

impl Form {
    fn unit_len(self) -> usize {
        match self {
            Form::Utf16Le | Form::Utf16Be => 2,
            Form::Utf32Le | Form::Utf32Be => 4,
        }
    }

    fn read(self, b: &[u8]) -> u32 {
        match self {
            Form::Utf16Le => u16::from_le_bytes([b[0], b[1]]) as u32,
            Form::Utf16Be => u16::from_be_bytes([b[0], b[1]]) as u32,
            Form::Utf32Le => u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            Form::Utf32Be => u32::from_be_bytes([b[0], b[1], b[2], b[3]]),
        }
    }
}

/// `src` をデコードし、使ったバイト数を返す（`last` でなければ途中の単位を残す）。
pub(crate) fn decode(form: Form, src: &[u8], last: bool, sink: &mut Sink<'_>) -> usize {
    let n = form.unit_len();
    let mut i = 0;
    let invalid = |sink: &mut Sink<'_>, bytes: &[u8]| {
        for &b in bytes {
            sink.invalid(b);
        }
    };
    while i + n <= src.len() {
        let u = form.read(&src[i..]);
        if n == 4 {
            if char::from_u32(u).is_some() {
                sink.push_cp(u);
            } else {
                invalid(sink, &src[i..i + 4]);
            }
            i += 4;
            continue;
        }
        match u {
            0xD800..=0xDBFF => {
                if i + 4 > src.len() {
                    if !last {
                        return i;
                    }
                    invalid(sink, &src[i..i + 2]);
                    i += 2;
                    continue;
                }
                let lo = form.read(&src[i + 2..]);
                if (0xDC00..=0xDFFF).contains(&lo) {
                    sink.push_cp(0x10000 + ((u - 0xD800) << 10) + (lo - 0xDC00));
                    i += 4;
                } else {
                    // 不対の上位サロゲート（次の単位は改めて読む）
                    invalid(sink, &src[i..i + 2]);
                    i += 2;
                }
            }
            0xDC00..=0xDFFF => {
                invalid(sink, &src[i..i + 2]);
                i += 2;
            }
            _ => {
                sink.push_cp(u);
                i += 2;
            }
        }
    }
    if last {
        invalid(sink, &src[i..]);
        return src.len();
    }
    i
}

pub(crate) fn encode(form: Form, s: &str, dst: &mut Vec<u8>) {
    match form {
        Form::Utf16Le | Form::Utf16Be => {
            dst.reserve(s.len() * 2);
            for u in s.encode_utf16() {
                let b = if form == Form::Utf16Le {
                    u.to_le_bytes()
                } else {
                    u.to_be_bytes()
                };
                dst.extend_from_slice(&b);
            }
        }
        Form::Utf32Le | Form::Utf32Be => {
            dst.reserve(s.len() * 4);
            for c in s.chars() {
                let b = if form == Form::Utf32Le {
                    (c as u32).to_le_bytes()
                } else {
                    (c as u32).to_be_bytes()
                };
                dst.extend_from_slice(&b);
            }
        }
    }
}
