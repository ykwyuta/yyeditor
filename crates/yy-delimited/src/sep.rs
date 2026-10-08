//! 区切り文字・レコードの終わりの書き方（制御コードを含む）。
//!
//! 文字はそのまま（`,` `|` `;;`）、制御コードは `<US>`・`<RS>`・`<TAB>`・`<CR>`・`<LF>`・`<NUL>` などの名前か
//! `<0x1F>`、または `\t`・`\r`・`\n`・`\0`・`\x1F`・`\\` で書く。

/// 制御コード（0x00〜0x1F）の名前。
const NAMES: [&str; 32] = [
    "NUL", "SOH", "STX", "ETX", "EOT", "ENQ", "ACK", "BEL", "BS", "TAB", "LF", "VT", "FF", "CR",
    "SO", "SI", "DLE", "DC1", "DC2", "DC3", "DC4", "NAK", "SYN", "ETB", "CAN", "EM", "SUB", "ESC",
    "FS", "GS", "RS", "US",
];

fn named(name: &str) -> Option<u8> {
    let up = name.trim().to_ascii_uppercase();
    if let Some(i) = NAMES.iter().position(|n| *n == up) {
        return Some(i as u8);
    }
    match up.as_str() {
        "HT" => Some(b'\t'),
        "NL" => Some(b'\n'),
        "SP" | "SPACE" => Some(b' '),
        "DEL" => Some(0x7F),
        _ => {
            let hex = up.strip_prefix("0X")?;
            u8::from_str_radix(hex, 16).ok()
        }
    }
}

/// 書き方をバイト列にする（空・正しくなければエラー）。
pub fn parse_bytes(text: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut it = text.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '<' => {
                let mut name = String::new();
                let mut closed = false;
                for c in it.by_ref() {
                    if c == '>' {
                        closed = true;
                        break;
                    }
                    name.push(c);
                }
                if !closed {
                    return Err(format!("「<」に対応する「>」がありません: {text}"));
                }
                out.push(named(&name).ok_or_else(|| format!("知らない名前です: <{name}>"))?);
            }
            '\\' => match it.next() {
                Some('t') => out.push(b'\t'),
                Some('r') => out.push(b'\r'),
                Some('n') => out.push(b'\n'),
                Some('0') => out.push(0),
                Some('\\') => out.push(b'\\'),
                Some('x') | Some('X') => {
                    let h: String = [it.next(), it.next()].into_iter().flatten().collect();
                    let v = u8::from_str_radix(&h, 16)
                        .ok()
                        .filter(|_| h.len() == 2)
                        .ok_or_else(|| {
                            format!("\\x の後には 16 進数 2 桁を書いてください: {text}")
                        })?;
                    out.push(v);
                }
                other => {
                    return Err(format!(
                        "知らない書き方です: \\{}",
                        other.map(String::from).unwrap_or_default()
                    ));
                }
            },
            c => {
                let mut b = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut b).as_bytes());
            }
        }
    }
    if out.is_empty() {
        return Err("区切りを入力してください".into());
    }
    Ok(out)
}

/// バイト列を読める形にする（制御コードは `<US>` など。[`parse_bytes`] で元に戻る）。
pub fn describe_bytes(bytes: &[u8]) -> String {
    let mut out = String::new();
    let text = String::from_utf8_lossy(bytes);
    for c in text.chars() {
        match c {
            '\u{0}'..='\u{1f}' => {
                out.push('<');
                out.push_str(NAMES[c as usize]);
                out.push('>');
            }
            '\u{7f}' => out.push_str("<DEL>"),
            '<' => out.push_str("<0x3C>"),
            '\\' => out.push_str("\\\\"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_names_escapes_and_text() {
        assert_eq!(parse_bytes(",").unwrap(), b",");
        assert_eq!(parse_bytes("<US>").unwrap(), b"\x1f");
        assert_eq!(parse_bytes("<rs>").unwrap(), b"\x1e");
        assert_eq!(parse_bytes("<CR><LF>").unwrap(), b"\r\n");
        assert_eq!(parse_bytes("<0x1d>").unwrap(), b"\x1d");
        assert_eq!(parse_bytes("\\t|\\x1F\\0\\\\").unwrap(), b"\t|\x1f\0\\");
        assert_eq!(parse_bytes("、").unwrap(), "、".as_bytes());
        assert!(parse_bytes("").is_err());
        assert!(parse_bytes("<XYZ>").is_err());
        assert!(parse_bytes("<US").is_err());
        assert!(parse_bytes("\\x1").is_err());
        assert!(parse_bytes("\\q").is_err());
    }

    #[test]
    fn describes_and_round_trips() {
        assert_eq!(describe_bytes(b"\x1f"), "<US>");
        assert_eq!(describe_bytes(b"\r\n"), "<CR><LF>");
        assert_eq!(describe_bytes(b"|"), "|");
        for b in [&b"\x1e"[..], b"\t", b"<>", b"a\\b", b"\x00\x7f", b";;"] {
            assert_eq!(parse_bytes(&describe_bytes(b)).unwrap(), b, "{b:?}");
        }
    }
}
