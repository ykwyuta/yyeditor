//! カーソル移動（文書上の位置の計算）。
//!
//! 画面の表示行に依存する上下移動は UI 層（DirectWrite のヒットテスト）で行い、
//! ここでは文字・単語・論理行単位の移動を扱う。
//! 巨大ファイルでも一定の範囲だけを読むよう、探索範囲には上限を設ける。

use unicode_segmentation::UnicodeSegmentation;
use yy_buffer::Snapshot;

/// 書記素クラスタの探索で読む範囲（これより長いクラスタは途中で区切る）
const GRAPHEME_WINDOW: u64 = 256;
/// 単語移動で読む範囲
const WORD_WINDOW: u64 = 4096;

/// UTF-8 として有効な先頭部分を文字列として返す。
fn valid_prefix(bytes: &[u8]) -> &str {
    match std::str::from_utf8(bytes) {
        Ok(s) => s,
        // SAFETY 不要: valid_up_to までは有効な UTF-8
        Err(e) => std::str::from_utf8(&bytes[..e.valid_up_to()]).unwrap(),
    }
}

/// UTF-8 として有効な末尾部分と、その開始位置（`bytes` 内）を返す。
fn valid_suffix(bytes: &[u8]) -> (usize, &str) {
    // 後ろから有効な UTF-8 が続く最長の範囲を探す
    let mut start = bytes.len();
    loop {
        if start == 0 {
            break;
        }
        // 1 文字分（最大 4 バイト）戻れるか試す
        let mut ok = None;
        for k in 1..=4.min(start) {
            if std::str::from_utf8(&bytes[start - k..start]).is_ok() {
                ok = Some(k);
                break;
            }
        }
        match ok {
            Some(k) => start -= k,
            None => break,
        }
    }
    (start, std::str::from_utf8(&bytes[start..]).unwrap())
}

/// `offset` の次の書記素クラスタ境界。不正なバイトは 1 バイトを 1 単位とする。
pub fn next_grapheme(snap: &Snapshot, offset: u64) -> u64 {
    let len = snap.len();
    if offset >= len {
        return len;
    }
    let bytes = snap.read(offset..(offset + GRAPHEME_WINDOW).min(len));
    let s = valid_prefix(&bytes);
    match s.grapheme_indices(true).nth(1) {
        Some((i, _)) => offset + i as u64,
        None if !s.is_empty() => offset + s.len() as u64,
        None => offset + 1,
    }
}

/// `offset` の前の書記素クラスタ境界。
pub fn prev_grapheme(snap: &Snapshot, offset: u64) -> u64 {
    let offset = offset.min(snap.len());
    if offset == 0 {
        return 0;
    }
    let from = offset.saturating_sub(GRAPHEME_WINDOW);
    let bytes = snap.read(from..offset);
    let (start, s) = valid_suffix(&bytes);
    match s.grapheme_indices(true).next_back() {
        Some((i, _)) => from + (start + i) as u64,
        None => offset - 1,
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Class {
    Space,
    Newline,
    /// 英数字・アンダースコア（ラテン文字等を含む）
    Word,
    Kanji,
    Hiragana,
    Katakana,
    Punct,
    Invalid,
}

fn class_of(c: char) -> Class {
    match c {
        '\n' | '\r' => Class::Newline,
        c if c.is_whitespace() => Class::Space,
        '_' => Class::Word,
        '\u{3041}'..='\u{309F}' => Class::Hiragana,
        '\u{30A0}'..='\u{30FF}' | '\u{31F0}'..='\u{31FF}' | '\u{FF66}'..='\u{FF9F}' => {
            Class::Katakana
        }
        '\u{3400}'..='\u{4DBF}'
        | '\u{4E00}'..='\u{9FFF}'
        | '\u{F900}'..='\u{FAFF}'
        | '々'
        | '〆' => Class::Kanji,
        c if c.is_alphanumeric() => Class::Word,
        _ => Class::Punct,
    }
}

/// 位置 `offset` から前方の文字とその長さを返す。
fn char_at(snap: &Snapshot, offset: u64) -> Option<(Class, u64)> {
    let len = snap.len();
    if offset >= len {
        return None;
    }
    let bytes = snap.read(offset..(offset + 4).min(len));
    match valid_prefix(&bytes).chars().next() {
        Some(c) => Some((class_of(c), c.len_utf8() as u64)),
        None => Some((Class::Invalid, 1)),
    }
}

fn char_before(snap: &Snapshot, offset: u64) -> Option<(Class, u64)> {
    if offset == 0 {
        return None;
    }
    let bytes = snap.read(offset.saturating_sub(4)..offset);
    let (_, s) = valid_suffix(&bytes);
    match s.chars().next_back() {
        Some(c) => Some((class_of(c), c.len_utf8() as u64)),
        None => Some((Class::Invalid, 1)),
    }
}

/// 次の単語の先頭（Ctrl+→）。同じ文字種の並びを飛ばし、続く空白も飛ばす。改行は 1 単位。
pub fn next_word(snap: &Snapshot, offset: u64) -> u64 {
    let limit = offset.saturating_add(WORD_WINDOW).min(snap.len());
    let Some((first, n)) = char_at(snap, offset) else {
        return offset;
    };
    if first == Class::Newline {
        return next_grapheme(snap, offset);
    }
    let mut pos = offset + n;
    if first != Class::Space {
        while pos < limit {
            match char_at(snap, pos) {
                Some((c, n)) if c == first => pos += n,
                _ => break,
            }
        }
    }
    while pos < limit {
        match char_at(snap, pos) {
            Some((Class::Space, n)) => pos += n,
            _ => break,
        }
    }
    pos
}

/// 前の単語の先頭（Ctrl+←）。
pub fn prev_word(snap: &Snapshot, offset: u64) -> u64 {
    let limit = offset.saturating_sub(WORD_WINDOW);
    let mut pos = offset;
    while pos > limit {
        match char_before(snap, pos) {
            Some((Class::Space, n)) => pos -= n,
            _ => break,
        }
    }
    let Some((cls, _)) = char_before(snap, pos) else {
        return pos;
    };
    if cls == Class::Newline {
        return if pos == offset {
            prev_grapheme(snap, pos)
        } else {
            pos
        };
    }
    while pos > limit {
        match char_before(snap, pos) {
            Some((c, n)) if c == cls => pos -= n,
            _ => break,
        }
    }
    pos
}

/// `offset` を含む単語の範囲（ダブルクリックでの選択用）。
pub fn word_range(snap: &Snapshot, offset: u64) -> std::ops::Range<u64> {
    let Some((cls, n)) = char_at(snap, offset).or_else(|| char_before(snap, offset)) else {
        return offset..offset;
    };
    if cls == Class::Newline {
        return offset..offset;
    }
    let lo_limit = offset.saturating_sub(WORD_WINDOW);
    let hi_limit = offset.saturating_add(WORD_WINDOW).min(snap.len());
    let mut start = offset;
    while start > lo_limit {
        match char_before(snap, start) {
            Some((c, n)) if c == cls => start -= n,
            _ => break,
        }
    }
    let mut end = if char_at(snap, offset).is_some() {
        offset
    } else {
        offset - n
    };
    while end < hi_limit {
        match char_at(snap, end) {
            Some((c, n)) if c == cls => end += n,
            _ => break,
        }
    }
    start..end
}

/// `offset` を含む論理行の先頭。
pub fn line_start(snap: &Snapshot, offset: u64) -> u64 {
    let offset = offset.min(snap.len());
    snap.find_prev(0..offset, b'\n').map_or(0, |n| n + 1)
}

/// `offset` を含む論理行の末尾（改行文字の直前。CRLF なら CR の前）。
pub fn line_end(snap: &Snapshot, offset: u64) -> u64 {
    let len = snap.len();
    match snap.find_next(offset.min(len)..len, b'\n') {
        Some(n) if n > offset && snap.byte_at(n - 1) == Some(b'\r') => n - 1,
        Some(n) => n,
        None => len,
    }
}

/// スマートホーム: 行頭の空白の直後へ。すでにそこにいれば行頭へ。
pub fn smart_home(snap: &Snapshot, offset: u64) -> u64 {
    let ls = line_start(snap, offset);
    let end = line_end(snap, ls).min(ls + WORD_WINDOW);
    let bytes = snap.read(ls..end);
    let indent = bytes
        .iter()
        .take_while(|b| **b == b' ' || **b == b'\t')
        .count() as u64;
    let first = ls + indent;
    if offset == first { ls } else { first }
}

/// 行頭のインデント（空白・タブ）を返す（改行時の自動インデント用）。
pub fn indent_of_line(snap: &Snapshot, offset: u64) -> Vec<u8> {
    let ls = line_start(snap, offset);
    let end = offset.min(ls + WORD_WINDOW);
    snap.read(ls..end)
        .into_iter()
        .take_while(|b| *b == b' ' || *b == b'\t')
        .collect()
}

/// 行頭から `offset` までの文字数（ステータスバーの「列」表示用）。
/// 行頭が `limit` バイトより遠い場合は `None`。
pub fn column_of(snap: &Snapshot, offset: u64, limit: u64) -> Option<u64> {
    let from = offset.saturating_sub(limit);
    let ls = match snap.find_prev(from..offset, b'\n') {
        Some(n) => n + 1,
        None if from == 0 => 0,
        None => return None,
    };
    let bytes = snap.read(ls..offset);
    Some(bytes.iter().filter(|b| (**b & 0xC0) != 0x80).count() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(s: &str) -> Snapshot {
        Snapshot::from_bytes(s.as_bytes().to_vec())
    }

    #[test]
    fn graphemes_respect_crlf_combining_and_invalid_bytes() {
        let s = snap("a\r\nか\u{309A}👨\u{200D}👩b");
        let mut pos = 0;
        let mut stops = vec![pos];
        while pos < s.len() {
            pos = next_grapheme(&s, pos);
            stops.push(pos);
        }
        assert_eq!(stops, vec![0, 1, 3, 9, 20, 21]);
        let mut back = vec![s.len()];
        let mut pos = s.len();
        while pos > 0 {
            pos = prev_grapheme(&s, pos);
            back.push(pos);
        }
        back.reverse();
        assert_eq!(back, stops);

        let bad = Snapshot::from_bytes(b"a\xFF\xFEb".to_vec());
        assert_eq!(next_grapheme(&bad, 1), 2);
        assert_eq!(prev_grapheme(&bad, 3), 2);
        assert_eq!(prev_grapheme(&bad, 4), 3);
    }

    #[test]
    fn word_motion_uses_character_classes() {
        let s = snap("hello world  漢字かなカナ, end\nnext");
        let mut stops = vec![0];
        let mut pos = 0;
        while pos < s.len() {
            pos = next_word(&s, pos);
            stops.push(pos);
        }
        let texts: Vec<_> = stops
            .windows(2)
            .map(|w| String::from_utf8(s.read(w[0]..w[1])).unwrap())
            .collect();
        assert_eq!(
            texts,
            vec![
                "hello ", "world  ", "漢字", "かな", "カナ", ", ", "end", "\n", "next"
            ]
        );
        // 逆方向でも同じ区切り（空白は前の単語に付く）
        let mut back = vec![s.len()];
        let mut pos = s.len();
        while pos > 0 {
            pos = prev_word(&s, pos);
            back.push(pos);
        }
        back.reverse();
        assert_eq!(back, stops);
    }

    #[test]
    fn line_boundaries_and_smart_home() {
        let s = snap("  indented\r\nnext\n");
        assert_eq!(line_start(&s, 5), 0);
        assert_eq!(line_end(&s, 5), 10);
        assert_eq!(line_start(&s, 12), 12);
        assert_eq!(line_end(&s, 12), 16);
        assert_eq!(line_end(&s, 17), 17);
        assert_eq!(smart_home(&s, 5), 2);
        assert_eq!(smart_home(&s, 2), 0);
        assert_eq!(indent_of_line(&s, 8), b"  ");
        assert_eq!(column_of(&s, 14, 1000), Some(2));
        assert_eq!(word_range(&s, 4), 2..10);
    }
}
