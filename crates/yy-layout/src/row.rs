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
    /// `Row::text` 内の範囲
    pub range: Range<usize>,
    pub kind: SpanKind,
    /// 元のバイト列（行の内容の先頭からの相対位置）での範囲
    pub src: Range<usize>,
}

impl Span {
    /// 元の 1 バイトあたりの表示テキストのバイト数（Text は 1、制御文字は 3、不正バイトは 4）。
    fn ratio(&self) -> usize {
        match self.kind {
            SpanKind::Text => 1,
            _ => (self.range.len() / self.src.len().max(1)).max(1),
        }
    }
}

/// 1 表示行。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    /// 文書内の開始位置
    pub start: u64,
    /// 内容（改行を除く）の終了位置
    pub end: u64,
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
            end: start + content.len() as u64,
            next,
            line_start,
            ends_line,
            text,
            spans,
        }
    }

    /// 文書のオフセットを `text` 内のバイト位置に変換する（行の外は端に丸める）。
    pub fn text_index(&self, offset: u64) -> usize {
        let rel = (offset.clamp(self.start, self.end) - self.start) as usize;
        for sp in &self.spans {
            if sp.src.start <= rel && rel < sp.src.end {
                return sp.range.start + (rel - sp.src.start) * sp.ratio();
            }
        }
        self.text.len()
    }

    /// `text` 内のバイト位置を文書のオフセットに変換する。
    /// 置き換え表示（`\xNN` など）の途中は、その元のバイトの先頭に丸める。
    pub fn offset_at(&self, text_index: usize) -> u64 {
        for sp in &self.spans {
            if sp.range.start <= text_index && text_index < sp.range.end {
                let rel = sp.src.start + (text_index - sp.range.start) / sp.ratio();
                return self.start + rel as u64;
            }
        }
        self.end
    }

    /// カーソル位置 `offset` をこの行に表示するか。
    ///
    /// 長大行の分割境界（改行ではない行の終わり）は次の行に表示する。
    pub fn shows_caret(&self, offset: u64) -> bool {
        if self.ends_line || self.end == self.next {
            self.start <= offset && offset <= self.end
        } else {
            self.start <= offset && offset < self.next
        }
    }
}

fn push(text: &mut String, spans: &mut Vec<Span>, s: &str, kind: SpanKind, src: Range<usize>) {
    if s.is_empty() {
        return;
    }
    let start = text.len();
    text.push_str(s);
    match spans.last_mut() {
        Some(last)
            if last.kind == kind
                && last.range.end == start
                && last.src.end == src.start
                && (kind == SpanKind::Text || last.ratio() == s.len() / src.len().max(1)) =>
        {
            last.range.end = text.len();
            last.src.end = src.end;
        }
        _ => spans.push(Span {
            range: start..text.len(),
            kind,
            src,
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
    let mut pos = 0usize;
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
                push(
                    &mut text,
                    &mut spans,
                    &valid[run_start..i],
                    SpanKind::Text,
                    pos + run_start..pos + i,
                );
                let mut buf = [0u8; 4];
                push(
                    &mut text,
                    &mut spans,
                    pic.encode_utf8(&mut buf),
                    SpanKind::Control,
                    pos + i..pos + i + 1,
                );
                run_start = i + c.len_utf8();
            }
        }
        push(
            &mut text,
            &mut spans,
            &valid[run_start..],
            SpanKind::Text,
            pos + run_start..pos + valid.len(),
        );
        pos += valid.len();
        for b in chunk.invalid() {
            push(
                &mut text,
                &mut spans,
                &format!("\\x{b:02X}"),
                SpanKind::Invalid,
                pos..pos + 1,
            );
            pos += 1;
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

    #[test]
    fn maps_offsets_to_text_and_back() {
        // "aあ" + 不正バイト + 制御文字 + "b" + CRLF
        let r = Row::new(100, 110, true, b"a\xE3\x81\x82\xFF\x01b\r\n");
        assert_eq!(r.text, "aあ\\xFF\u{2401}b");
        assert_eq!(r.end, 107);
        let pairs: Vec<(u64, usize)> = (100..=107).map(|o| (o, r.text_index(o))).collect();
        assert_eq!(
            pairs,
            vec![
                (100, 0),
                (101, 1),
                (102, 2),
                (103, 3),
                (104, 4),
                (105, 8),
                (106, 11),
                (107, 12)
            ]
        );
        for (o, i) in [(100, 0), (101, 1), (104, 4), (105, 8), (106, 11), (107, 12)] {
            assert_eq!(r.offset_at(i), o);
        }
        // 置き換え表示の途中は元のバイトの先頭に丸める
        assert_eq!(r.offset_at(6), 104);
        assert!(r.shows_caret(107));
        assert!(!r.shows_caret(108));
    }
}
