//! Read-only decoded review with unambiguous visible byte/control escapes.
use crate::{Cancellation, MAX_BYTES, Result};
use serde::{Deserialize, Serialize};
use yy_encoding::{Encoding, decode_all, unescape_char};
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mode {
    pub ignore_encoding: bool,
}
#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct Info {
    pub escaped_bytes: u64,
    pub control_characters: u64,
    pub raw_fallback: bool,
}
fn push(out: &mut String, text: &str) -> Result<()> {
    if out.len() + text.len() > MAX_BYTES as usize {
        return Err("decoded byte review exceeds 32 MiB text limit".into());
    }
    out.push_str(text);
    Ok(())
}
pub fn decode(encoding: Encoding, bytes: &[u8], c: &Cancellation) -> Result<(String, Info)> {
    c.check()?;
    let (decoded, mut stats) = if encoding == Encoding::Utf8 {
        (Vec::new(), yy_encoding::DecodeStats::default())
    } else {
        decode_all(encoding, bytes, true)
    };
    // The shared UTF-8 decoder intentionally passes raw bytes through unchanged.
    let decoded = if encoding == Encoding::Utf8 {
        let mut out = Vec::new();
        let mut rest = bytes;
        while !rest.is_empty() {
            c.check()?;
            let (valid, bad) = match std::str::from_utf8(rest) {
                Ok(_) => (rest.len(), 0),
                Err(e) => (
                    e.valid_up_to(),
                    e.error_len().unwrap_or(rest.len() - e.valid_up_to()),
                ),
            };
            let text = std::str::from_utf8(&rest[..valid]).map_err(|e| e.to_string())?;
            stats.literal_escapes += text
                .chars()
                .filter(|ch| unescape_char(*ch).is_some())
                .count() as u64;
            out.extend_from_slice(&rest[..valid]);
            for byte in &rest[valid..valid + bad] {
                out.extend_from_slice(
                    yy_encoding::escape_char(*byte)
                        .encode_utf8(&mut [0; 4])
                        .as_bytes(),
                );
            }
            rest = &rest[valid + bad..];
        }
        out
    } else {
        decoded
    };
    let mut info = Info::default();
    let mut text = String::new();
    if stats.literal_escapes > 0 {
        // Literal private-use characters must never collide with escaped invalid bytes.
        info.raw_fallback = true;
        info.escaped_bytes = bytes.len() as u64;
        push(&mut text, "\\raw:\n")?;
        for (i, byte) in bytes.iter().enumerate() {
            if i & 1023 == 0 {
                c.check()?;
            }
            push(&mut text, &format!("\\x{byte:02X}"))?;
            if i % 16 == 15 {
                push(&mut text, "\n")?;
            }
        }
    } else {
        let decoded = std::str::from_utf8(&decoded).map_err(|_| "decoder did not produce UTF-8")?;
        for (i, ch) in decoded.chars().enumerate() {
            if i & 1023 == 0 {
                c.check()?;
            }
            if let Some(byte) = unescape_char(ch) {
                info.escaped_bytes += 1;
                push(&mut text, &format!("\\x{byte:02X}"))?;
            } else if ch == '\\' {
                push(&mut text, "\\\\")?;
            } else if ch.is_control() && !matches!(ch, '\r' | '\n' | '\t') {
                info.control_characters += 1;
                push(&mut text, &format!("\\u{{{:04X}}}", ch as u32))?;
            } else {
                push(&mut text, ch.encode_utf8(&mut [0; 4]))?;
            }
        }
    }
    c.check()?;
    Ok((text, info))
}

pub fn valid_encoding(name: &str) -> bool {
    Encoding::from_name(name).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn byte_review_accepts_packed_data_across_supported_mainframe_encodings() {
        let c = Cancellation::new();
        let bytes: Vec<u8> = (0..=255).collect();
        for name in [
            "IBM-037",
            "IBM-500",
            "IBM-1047",
            "IBM-290",
            "IBM-1027",
            "IBM-930",
            "IBM-939",
            "IBM-1390",
            "IBM-1399",
            "IBM-939/fixed:80",
            "UTF-8",
            "UTF-16LE",
            "UTF-32BE",
            "CP932",
            "ISO-2022-JP",
        ] {
            let enc = Encoding::from_name(name).unwrap();
            let (text, _) = decode(enc, &bytes, &c).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(!text.contains('\0'), "{name}");
            assert!(!text.is_empty(), "{name}");
        }
        let mut text = "a".repeat(MAX_BYTES as usize);
        assert!(push(&mut text, "b").is_err());
        assert_eq!(text.len(), MAX_BYTES as usize);
    }
    #[test]
    fn byte_review_is_unambiguous_and_cancelable() {
        let c = Cancellation::new();
        let (text, info) = decode(Encoding::Utf8, &[0x12, 0x3c, 0, 0xff], &c).unwrap();
        assert_eq!(text, "\\u{0012}<\\u{0000}\\xFF");
        assert_eq!(info.escaped_bytes, 1);
        assert_eq!(info.control_characters, 2);
        assert_ne!(
            text,
            decode(Encoding::Utf8, b"\\u{0012}<\\u{0000}\\xFF", &c)
                .unwrap()
                .0
        );
        assert_ne!(
            decode(Encoding::Utf8, &[0xfe], &c).unwrap().0,
            decode(Encoding::Utf8, &[0xff], &c).unwrap().0
        );
        let reserved = char::from_u32(0x10feff).unwrap().to_string();
        let (literal, info) = decode(Encoding::Utf8, reserved.as_bytes(), &c).unwrap();
        assert!(info.raw_fallback);
        assert_ne!(literal, decode(Encoding::Utf8, &[0xff], &c).unwrap().0);
        c.cancel();
        assert!(decode(Encoding::Utf8, b"abc", &c).is_err());
    }
    #[test]
    fn mixed_encoding_sessions_retain_fixed_records_and_original_bytes() {
        use crate::{
            comparison_session::{Position, Session},
            files::Snapshot,
            rules::Rules,
        };
        use yy_encoding::{EscapeMode, encode_all};
        let dir = tempfile::tempdir().unwrap();
        let left = dir.path().join("left.dat");
        let right = dir.path().join("right.dat");
        let c = Cancellation::new();
        let enc = Encoding::from_name("IBM-939/fixed:8").unwrap();
        let original = encode_all(enc, b"ABCD1234\n", EscapeMode::Reject).unwrap();
        std::fs::write(&left, &original).unwrap();
        std::fs::write(&right, b"ABCD1234\n").unwrap();
        let mode = Mode {
            ignore_encoding: true,
        };
        let l = Snapshot::open_review(&left, Some(&enc.spec()), mode, &c).unwrap();
        let r = Snapshot::open_review(&right, Some("UTF-8"), mode, &c).unwrap();
        assert_eq!(l.text, r.text);
        assert!(l.equivalent(&r, true));
        assert!(!l.original_identical(&r));
        assert!(l.encode(&l.text).is_err());
        let state = Session::capture([&l, &r], Rules::default(), Position::default(), &c).unwrap();
        let path = dir.path().join("session.json");
        std::fs::write(&path, state.json(&c).unwrap()).unwrap();
        let (state, _) = Session::open(&path, &c).unwrap();
        assert_eq!(state.inputs[0].encoding, enc);
        assert_eq!(state.inputs[0].review, Some(mode));
        assert_eq!(state.inputs[0].text, l.text);
        assert_eq!(std::fs::read(&left).unwrap(), original);
        std::fs::write(&left, b"changed").unwrap();
        assert!(Session::open(&path, &c).is_err());
    }
}
