use std::ops::Range;

/// 表示行内の範囲の種類。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpanKind {
    Text,
    /// UTF-8 として不正なバイト（`\xNN` と表示）
    Invalid,
    /// 制御文字（Unicode の Control Pictures で表示）
    Control,
    /// 行・段落の区切りとして描画されてしまう文字（U+0085 NEL・U+2028・U+2029）。
    /// `␤` `¶` で表示する。1 つのスパンが 1 文字
    Break,
    /// 字形のない C1 制御文字（U+0080〜U+009F。U+0085 を除く）。`<94>` のように 16 進で表示する。
    /// 1 つのスパンが 1 文字で、表示の桁数は表示テキストの長さ
    Symbol,
    /// 読み込んだ文字コードで不正だったバイトを表すエスケープ文字（`\xNN` と表示。03 章 2.4）
    Escape,
    /// 区切り文字モードの区切り文字（` │ ` と表示。04 章 4.2）。1 つのスパンが 1 単位
    Delim,
    /// 区切り文字モードで列を揃えるための空白（元のバイト列には対応しない）
    Pad,
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
    /// 1 単位の（元のバイト数, 表示テキストのバイト数）。置き換え表示は単位の途中で区切らない。
    fn unit(&self) -> (usize, usize) {
        match self.kind {
            SpanKind::Text => (1, 1),
            SpanKind::Invalid => (1, 4),
            SpanKind::Control => (1, 3),
            SpanKind::Break | SpanKind::Symbol => (self.src.len().max(1), self.range.len().max(1)),
            SpanKind::Escape => (4, 4),
            SpanKind::Delim => (self.src.len().max(1), self.range.len().max(1)),
            SpanKind::Pad => (0, 1),
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
    #[cfg(test)]
    pub(crate) fn new(start: u64, next: u64, line_start: bool, bytes: &[u8]) -> Row {
        Self::new_with_controls(start, next, line_start, bytes, true)
    }

    pub(crate) fn new_with_controls(
        start: u64,
        next: u64,
        line_start: bool,
        bytes: &[u8],
        show_controls: bool,
    ) -> Row {
        let (content, ends_line) = match bytes {
            [rest @ .., b'\r', b'\n'] => (rest, true),
            [rest @ .., b'\n'] => (rest, true),
            _ => (bytes, false),
        };
        let (text, spans) = decode_row_with_controls(content, show_controls);
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
            // フィールドの終わりのカーソルは、列を揃える空白の手前に表示する
            if sp.kind == SpanKind::Pad && sp.src.start == rel && sp.range.start > 0 {
                return sp.range.start;
            }
            if sp.src.start <= rel && rel < sp.src.end {
                let (us, ut) = sp.unit();
                return sp.range.start + (rel - sp.src.start) / us * ut;
            }
        }
        self.text.len()
    }

    /// `text` 内のバイト位置を文書のオフセットに変換する。
    /// 置き換え表示（`\xNN` など）の途中は、その元のバイトの先頭に丸める。
    pub fn offset_at(&self, text_index: usize) -> u64 {
        for sp in &self.spans {
            if sp.range.start <= text_index && text_index < sp.range.end {
                let (us, ut) = sp.unit();
                let rel = sp.src.start + (text_index - sp.range.start) / ut * us;
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

/// `bytes`（行の内容の `base` バイト目から）を表示用テキストにして追加する。
pub(crate) fn append_decoded(
    text: &mut String,
    spans: &mut Vec<Span>,
    bytes: &[u8],
    base: usize,
    show_controls: bool,
) {
    let (t, sp) = decode_row_with_controls(bytes, show_controls);
    let off = text.len();
    for s in sp {
        push(
            text,
            spans,
            &t[s.range.clone()],
            s.kind,
            s.src.start + base..s.src.end + base,
        );
    }
    debug_assert_eq!(text.len(), off + t.len());
}

/// 表示用のスパン（区切り文字・空白など）を追加する。
pub(crate) fn push_span(
    text: &mut String,
    spans: &mut Vec<Span>,
    s: &str,
    kind: SpanKind,
    src: Range<usize>,
) {
    push(text, spans, s, kind, src);
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
                && !matches!(
                    kind,
                    SpanKind::Delim | SpanKind::Pad | SpanKind::Break | SpanKind::Symbol
                )
                && last.range.end == start
                && last.src.end == src.start =>
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
/// * U+0085（NEL、EBCDIC の改行）・U+2028・U+2029 は `␤` `¶` に置き換える
///   （そのまま描画すると行が分かれてしまうため）
/// * そのほかの C1 制御文字（U+0080〜U+009F）は `<94>` のように置き換える（字形がなく見えないため。
///   EBCDIC のパック 10 進数・2 進数の項目に多い）
/// * エスケープ文字（`U+10FE00`〜）は元のバイトを `\xNN` で表示する
pub fn decode_row(bytes: &[u8]) -> (String, Vec<Span>) {
    decode_row_with_controls(bytes, true)
}

pub fn decode_row_with_controls(bytes: &[u8], show_controls: bool) -> (String, Vec<Span>) {
    // よくある場合（制御文字・エスケープ文字を含まない正しい UTF-8）はそのまま使う
    // （64 バイトずつ分岐なしで調べてベクトル化させる）
    let plain = bytes.chunks(64).all(|c| {
        let mut bad = false;
        for &b in c {
            bad |=
                ((b < 0x20) & (b != b'\t')) | (b == 0x7F) | (b == 0xF4) | (b == 0xC2) | (b == 0xE2);
        }
        !bad
    });
    if plain && let Ok(s) = std::str::from_utf8(bytes) {
        let spans = if s.is_empty() {
            Vec::new()
        } else {
            vec![Span {
                range: 0..s.len(),
                kind: SpanKind::Text,
                src: 0..s.len(),
            }]
        };
        return (s.to_owned(), spans);
    }
    let mut text = String::with_capacity(bytes.len());
    let mut spans = Vec::new();
    let mut pos = 0usize;
    for chunk in bytes.utf8_chunks() {
        let valid = chunk.valid();
        let mut run_start = 0;
        for (i, c) in valid.char_indices() {
            if let Some(b) = yy_encoding::unescape_char(c) {
                push(
                    &mut text,
                    &mut spans,
                    &valid[run_start..i],
                    SpanKind::Text,
                    pos + run_start..pos + i,
                );
                push(
                    &mut text,
                    &mut spans,
                    &format!("\\x{b:02X}"),
                    SpanKind::Escape,
                    pos + i..pos + i + 4,
                );
                run_start = i + 4;
                continue;
            }
            let brk = match c {
                '\u{85}' | '\u{2028}' => Some("\u{2424}"),
                '\u{2029}' => Some("\u{B6}"),
                _ => None,
            };
            if let Some(glyph) = brk {
                push(
                    &mut text,
                    &mut spans,
                    &valid[run_start..i],
                    SpanKind::Text,
                    pos + run_start..pos + i,
                );
                let glyph = if show_controls { glyph } else { " " };
                push(
                    &mut text,
                    &mut spans,
                    glyph,
                    SpanKind::Break,
                    pos + i..pos + i + c.len_utf8(),
                );
                run_start = i + c.len_utf8();
                continue;
            }
            if ('\u{80}'..='\u{9F}').contains(&c) {
                push(
                    &mut text,
                    &mut spans,
                    &valid[run_start..i],
                    SpanKind::Text,
                    pos + run_start..pos + i,
                );
                let label = if show_controls {
                    format!("<{:02X}>", c as u32)
                } else {
                    " ".to_owned()
                };
                push(
                    &mut text,
                    &mut spans,
                    &label,
                    SpanKind::Symbol,
                    pos + i..pos + i + c.len_utf8(),
                );
                run_start = i + c.len_utf8();
                continue;
            }
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
                let glyph = if show_controls {
                    pic.encode_utf8(&mut buf)
                } else {
                    " "
                };
                push(
                    &mut text,
                    &mut spans,
                    glyph,
                    if show_controls {
                        SpanKind::Control
                    } else {
                        SpanKind::Text
                    },
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
    fn line_separators_do_not_break_rows() {
        // NEL（2 バイト）と U+2028（3 バイト）が続く
        let bytes = "a\u{85}\u{2028}b".as_bytes();
        let r = Row::new(0, bytes.len() as u64, true, bytes);
        assert_eq!(r.text, "a\u{2424}\u{2424}b");
        assert_eq!(r.spans.len(), 4);
        assert_eq!(r.text_index(1), 1);
        assert_eq!(r.text_index(3), 4);
        assert_eq!(r.text_index(6), 7);
        assert_eq!(r.offset_at(4), 3);
        assert_eq!(r.offset_at(7), 6);
        let hidden = Row::new_with_controls(0, bytes.len() as u64, true, bytes, false);
        assert_eq!(hidden.text, "a  b");
        assert_eq!(hidden.offset_at(3), 6);
    }

    #[test]
    fn control_display_can_be_hidden_without_changing_offsets() {
        let shown = Row::new_with_controls(0, 3, true, b"a\x01b", true);
        let hidden = Row::new_with_controls(0, 3, true, b"a\x01b", false);
        assert_eq!(shown.text, "a\u{2401}b");
        assert_eq!(hidden.text, "a b");
        assert_eq!(hidden.text_index(2), 2);
        assert_eq!(hidden.offset_at(1), 1);
    }

    #[test]
    fn c1_controls_are_shown_as_hex() {
        // パック 10 進数 12 34 5C（IBM-037）= U+0012 U+0094 '*'、続けて C1 の U+0080・U+009F
        let bytes = "\u{12}\u{94}*\u{80}\u{9F}".as_bytes();
        let r = Row::new(0, bytes.len() as u64, true, bytes);
        assert_eq!(r.text, "\u{2412}<94>*<80><9F>");
        let kinds: Vec<_> = r.spans.iter().map(|s| s.kind).collect();
        assert_eq!(
            kinds,
            [
                SpanKind::Control,
                SpanKind::Symbol,
                SpanKind::Text,
                SpanKind::Symbol,
                SpanKind::Symbol
            ]
        );
        // 表示の途中は元の文字の先頭に丸める
        assert_eq!(r.text_index(1), 3);
        assert_eq!(r.text_index(3), 7);
        for i in 3..7 {
            assert_eq!(r.offset_at(i), 1);
        }
        assert_eq!(r.offset_at(7), 3);
        let cfg = crate::ColumnConfig::default();
        let cols: Vec<_> = crate::columns::units(&r, &cfg)
            .iter()
            .map(|u| (u.start, u.col_start, u.col_end))
            .collect();
        assert_eq!(
            cols,
            [(0, 0, 1), (1, 1, 5), (3, 5, 6), (4, 6, 10), (6, 10, 14)]
        );
        let hidden = Row::new_with_controls(0, bytes.len() as u64, true, bytes, false);
        assert_eq!(hidden.text, "  *  ");
    }

    #[test]
    fn escape_characters_are_atomic() {
        // "a" + エスケープ文字（0x87）+ "b"
        let mut bytes = b"a".to_vec();
        bytes.extend_from_slice(yy_encoding::escape_char(0x87).to_string().as_bytes());
        bytes.push(b'b');
        let r = Row::new(10, 16, true, &bytes);
        assert_eq!(r.text, "a\\x87b");
        assert_eq!(r.spans[1].kind, SpanKind::Escape);
        assert_eq!(r.text_index(11), 1);
        assert_eq!(r.text_index(15), 5);
        // 表示の途中は元の文字の先頭に丸める
        for i in 1..5 {
            assert_eq!(r.offset_at(i), 11);
        }
        assert_eq!(r.offset_at(5), 15);
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
