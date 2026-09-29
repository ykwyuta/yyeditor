use std::ops::Range;

/// 表示行内の範囲の種類。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpanKind {
    Text,
    /// UTF-8 として不正なバイト（`\xNN` と表示）
    Invalid,
    /// 制御文字（Unicode の Control Pictures で表示）
    Control,
}

/// 表示テキスト `Row::text` 内のバイト範囲と種類。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub range: Range<usize>,
    pub kind: SpanKind,
}

/// 1 表示行。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    /// 文書内の開始位置
    pub start: u64,
    /// 次の表示行の開始位置（最後の行なら文書長）
    pub next: u64,
    /// 論理行の先頭か（行番号を表示するか）
    pub line_start: bool,
    /// 改行で終わるか（論理行の末尾の表示行か）
    pub ends_line: bool,
    /// 表示用テキスト（改行を除き、不正バイト・制御文字を置き換えたもの）
    pub text: String,
    /// `text` 全体を覆う連続した範囲の列
    pub spans: Vec<Span>,
}

impl Row {
    pub(crate) fn new(start: u64, next: u64, line_start: bool, bytes: &[u8]) -> Row {
        let (content, ends_line) = match bytes {
            [rest @ .., b'\r', b'\n'] => (rest, true),
            [rest @ .., b'\n'] => (rest, true),
            _ => (bytes, false),
        };
        let (text, spans) = decode_row(content);
        Row {
            start,
            next,
            line_start,
            ends_line,
            text,
            spans,
        }
    }
}

fn push(text: &mut String, spans: &mut Vec<Span>, s: &str, kind: SpanKind) {
    if s.is_empty() {
        return;
    }
    let start = text.len();
    text.push_str(s);
    match spans.last_mut() {
        Some(last) if last.kind == kind && last.range.end == start => last.range.end = text.len(),
        _ => spans.push(Span {
            range: start..text.len(),
            kind,
        }),
    }
}

/// バイト列を表示用テキストに変換する。
///
/// * 不正な UTF-8 のバイトは `\xNN` に置き換える
/// * タブ以外の C0 制御文字と DEL は Control Pictures（`␀` `␍` など）に置き換える
pub fn decode_row(bytes: &[u8]) -> (String, Vec<Span>) {
    let mut text = String::with_capacity(bytes.len());
    let mut spans = Vec::new();
    for chunk in bytes.utf8_chunks() {
        let valid = chunk.valid();
        let mut run_start = 0;
        for (i, c) in valid.char_indices() {
            let pic = match c {
                '\t' => None,
                '\0'..='\x1F' => char::from_u32(0x2400 + c as u32),
                '\x7F' => Some('\u{2421}'),
                _ => None,
            };
            if let Some(pic) = pic {
                push(&mut text, &mut spans, &valid[run_start..i], SpanKind::Text);
                let mut buf = [0u8; 4];
                push(
                    &mut text,
                    &mut spans,
                    pic.encode_utf8(&mut buf),
                    SpanKind::Control,
                );
                run_start = i + c.len_utf8();
            }
        }
        push(&mut text, &mut spans, &valid[run_start..], SpanKind::Text);
        for b in chunk.invalid() {
            push(
                &mut text,
                &mut spans,
                &format!("\\x{b:02X}"),
                SpanKind::Invalid,
            );
        }
    }
    (text, spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_text_invalid_and_control() {
        let (t, s) = decode_row(b"ab\xFFc\x01\td\xE3\x81");
        assert_eq!(t, "ab\\xFFc\u{2401}\td\\xE3\\x81");
        let kinds: Vec<_> = s.iter().map(|s| (&t[s.range.clone()], s.kind)).collect();
        assert_eq!(
            kinds,
            vec![
                ("ab", SpanKind::Text),
                ("\\xFF", SpanKind::Invalid),
                ("c", SpanKind::Text),
                ("\u{2401}", SpanKind::Control),
                ("\td", SpanKind::Text),
                ("\\xE3\\x81", SpanKind::Invalid),
            ]
        );
    }

    #[test]
    fn strips_line_endings() {
        let r = Row::new(0, 5, true, b"abc\r\n");
        assert_eq!(r.text, "abc");
        assert!(r.ends_line);
        let r = Row::new(0, 4, true, b"a\rb");
        assert_eq!(r.text, "a\u{240D}b");
        assert!(!r.ends_line);
    }
}
