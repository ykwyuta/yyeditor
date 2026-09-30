//! 選択範囲の文字列の変換（大文字・小文字、カタカナの全角・半角、識別子の書き方）と、
//! 行の重複の除去。

use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::io::Write;

use yy_buffer::Snapshot;
use yy_search::ReplaceError;

/// 文字列の変換の種類。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transform {
    /// 英字を大文字に
    Upper,
    /// 英字を小文字に
    Lower,
    /// 全角カタカナを半角に
    HalfKatakana,
    /// 半角カタカナを全角に
    FullKatakana,
    /// `camelCase`
    Camel,
    /// `snake_case`
    Snake,
    /// `kebab-case`
    Kebab,
}

impl Transform {
    pub fn apply(self, text: &str) -> String {
        match self {
            Transform::Upper => map_latin(text, true),
            Transform::Lower => map_latin(text, false),
            Transform::HalfKatakana => to_half_katakana(text),
            Transform::FullKatakana => to_full_katakana(text),
            Transform::Camel => convert_identifiers(text, IdentCase::Camel),
            Transform::Snake => convert_identifiers(text, IdentCase::Snake),
            Transform::Kebab => convert_identifiers(text, IdentCase::Kebab),
        }
    }
}

// ---- 大文字・小文字 ------------------------------------------------------------

/// 英字（ASCII・ラテン文字・全角英字）か。
fn is_latin_letter(c: char) -> bool {
    c.is_ascii_alphabetic()
        || matches!(c, '\u{00C0}'..='\u{024F}' | 'Ａ'..='Ｚ' | 'ａ'..='ｚ') && c.is_alphabetic()
}

fn map_latin(text: &str, upper: bool) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if is_latin_letter(c) {
            if upper {
                out.extend(c.to_uppercase());
            } else {
                out.extend(c.to_lowercase());
            }
        } else {
            out.push(c);
        }
    }
    out
}

// ---- カタカナの全角・半角 --------------------------------------------------------

/// 半角カタカナ（U+FF61〜U+FF9F）と対応する全角の文字。
const HALF_TO_FULL: [(char, char); 63] = [
    ('｡', '。'),
    ('｢', '「'),
    ('｣', '」'),
    ('､', '、'),
    ('･', '・'),
    ('ｦ', 'ヲ'),
    ('ｧ', 'ァ'),
    ('ｨ', 'ィ'),
    ('ｩ', 'ゥ'),
    ('ｪ', 'ェ'),
    ('ｫ', 'ォ'),
    ('ｬ', 'ャ'),
    ('ｭ', 'ュ'),
    ('ｮ', 'ョ'),
    ('ｯ', 'ッ'),
    ('ｰ', 'ー'),
    ('ｱ', 'ア'),
    ('ｲ', 'イ'),
    ('ｳ', 'ウ'),
    ('ｴ', 'エ'),
    ('ｵ', 'オ'),
    ('ｶ', 'カ'),
    ('ｷ', 'キ'),
    ('ｸ', 'ク'),
    ('ｹ', 'ケ'),
    ('ｺ', 'コ'),
    ('ｻ', 'サ'),
    ('ｼ', 'シ'),
    ('ｽ', 'ス'),
    ('ｾ', 'セ'),
    ('ｿ', 'ソ'),
    ('ﾀ', 'タ'),
    ('ﾁ', 'チ'),
    ('ﾂ', 'ツ'),
    ('ﾃ', 'テ'),
    ('ﾄ', 'ト'),
    ('ﾅ', 'ナ'),
    ('ﾆ', 'ニ'),
    ('ﾇ', 'ヌ'),
    ('ﾈ', 'ネ'),
    ('ﾉ', 'ノ'),
    ('ﾊ', 'ハ'),
    ('ﾋ', 'ヒ'),
    ('ﾌ', 'フ'),
    ('ﾍ', 'ヘ'),
    ('ﾎ', 'ホ'),
    ('ﾏ', 'マ'),
    ('ﾐ', 'ミ'),
    ('ﾑ', 'ム'),
    ('ﾒ', 'メ'),
    ('ﾓ', 'モ'),
    ('ﾔ', 'ヤ'),
    ('ﾕ', 'ユ'),
    ('ﾖ', 'ヨ'),
    ('ﾗ', 'ラ'),
    ('ﾘ', 'リ'),
    ('ﾙ', 'ル'),
    ('ﾚ', 'レ'),
    ('ﾛ', 'ロ'),
    ('ﾜ', 'ワ'),
    ('ﾝ', 'ン'),
    ('ﾞ', '゛'),
    ('ﾟ', '゜'),
];

fn half_to_full(c: char) -> Option<char> {
    HALF_TO_FULL.iter().find(|(h, _)| *h == c).map(|(_, f)| *f)
}

/// 全角カタカナの濁音・半濁音 → （清音, 濁点か半濁点か）。
fn decompose_voiced(c: char) -> Option<(char, char)> {
    let v = c as u32;
    // ガ〜ド・バ〜ポ: 清音 + 1 が濁音、+ 2 が半濁音（ハ行）
    let seion = |d: u32| char::from_u32(v - d).unwrap();
    match c {
        'ガ' | 'ギ' | 'グ' | 'ゲ' | 'ゴ' | 'ザ' | 'ジ' | 'ズ' | 'ゼ' | 'ゾ' | 'ダ' | 'ヂ'
        | 'ヅ' | 'デ' | 'ド' | 'バ' | 'ビ' | 'ブ' | 'ベ' | 'ボ' => Some((seion(1), 'ﾞ')),
        'パ' | 'ピ' | 'プ' | 'ペ' | 'ポ' => Some((seion(2), 'ﾟ')),
        'ヴ' => Some(('ウ', 'ﾞ')),
        'ヷ' => Some(('ワ', 'ﾞ')),
        'ヺ' => Some(('ヲ', 'ﾞ')),
        _ => None,
    }
}

/// 清音と濁点・半濁点を合わせた全角の文字。
fn compose_voiced(base: char, mark: char) -> Option<char> {
    let v = base as u32;
    match (mark, base) {
        ('ﾞ', 'カ'..='ト') | ('ﾞ', 'ハ'..='ホ') => {
            // カ・キ…は清音の次が濁音（ッ・ツなどの小書きの位置も含めて並ぶ）
            let voiced = char::from_u32(v + 1)?;
            decompose_voiced(voiced).filter(|(b, _)| *b == base)?;
            Some(voiced)
        }
        ('ﾟ', 'ハ'..='ホ') => {
            let p = char::from_u32(v + 2)?;
            decompose_voiced(p).filter(|(b, m)| *b == base && *m == 'ﾟ')?;
            Some(p)
        }
        ('ﾞ', 'ウ') => Some('ヴ'),
        ('ﾞ', 'ワ') => Some('ヷ'),
        ('ﾞ', 'ヲ') => Some('ヺ'),
        _ => None,
    }
}

/// 全角カタカナ（と長音・中黒・濁点）を半角にする。半角にない文字（ヰ・ヱ など）はそのまま。
pub fn to_half_katakana(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        let (base, mark) = match decompose_voiced(c) {
            Some((b, m)) => (b, Some(m)),
            None => (c, None),
        };
        let is_kana_like = matches!(base, '\u{30A1}'..='\u{30FC}' | '゛' | '゜');
        match HALF_TO_FULL
            .iter()
            .find(|(_, f)| *f == base)
            .filter(|_| is_kana_like)
        {
            Some((h, _)) => {
                out.push(*h);
                out.extend(mark);
            }
            None => out.push(c),
        }
    }
    out
}

/// 半角カタカナを全角にする（濁点・半濁点は前の文字と合わせる）。
pub fn to_full_katakana(text: &str) -> String {
    let mut out: Vec<char> = Vec::with_capacity(text.len());
    let mut prev_was_half = false;
    for c in text.chars() {
        let Some(full) = half_to_full(c) else {
            out.push(c);
            prev_was_half = false;
            continue;
        };
        if matches!(c, 'ﾞ' | 'ﾟ')
            && prev_was_half
            && let Some(composed) = out.last().and_then(|&b| compose_voiced(b, c))
        {
            *out.last_mut().unwrap() = composed;
            prev_was_half = false;
            continue;
        }
        out.push(full);
        prev_was_half = true;
    }
    out.into_iter().collect()
}

// ---- 識別子の書き方 --------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum IdentCase {
    Camel,
    Snake,
    Kebab,
}

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

/// 識別子を単語に分ける（`_`・`-`・空白、小文字→大文字、`HTTPServer` の `P|S` の境目）。
fn split_words(ident: &str) -> Vec<String> {
    let chars: Vec<char> = ident.chars().collect();
    let mut words = Vec::new();
    let mut cur = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if c == '_' || c == '-' || c == ' ' {
            if !cur.is_empty() {
                words.push(std::mem::take(&mut cur));
            }
            continue;
        }
        if let Some(&p) = cur.chars().last().as_ref() {
            let next = chars.get(i + 1).copied();
            let boundary = (p.is_ascii_lowercase() || p.is_ascii_digit()) && c.is_ascii_uppercase()
                || p.is_ascii_uppercase()
                    && c.is_ascii_uppercase()
                    && next.is_some_and(|n| n.is_ascii_lowercase());
            if boundary {
                words.push(std::mem::take(&mut cur));
            }
        }
        cur.push(c);
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    words
}

/// 1 つの識別子を変換する（前後の `_`・`-` は残す）。
fn convert_ident(ident: &str, case: IdentCase) -> String {
    let core = ident.trim_matches(|c| c == '_' || c == '-');
    if core.is_empty() {
        return ident.to_owned();
    }
    let lead = &ident[..ident.find(core).unwrap_or(0)];
    let trail = &ident[lead.len() + core.len()..];
    let words = split_words(core);
    let body = match case {
        IdentCase::Camel => words
            .iter()
            .enumerate()
            .map(|(i, w)| {
                let lower = w.to_ascii_lowercase();
                if i == 0 {
                    lower
                } else {
                    let mut cs = lower.chars();
                    cs.next()
                        .map(|f| f.to_ascii_uppercase().to_string() + cs.as_str())
                        .unwrap_or_default()
                }
            })
            .collect::<String>(),
        IdentCase::Snake | IdentCase::Kebab => {
            let sep = if case == IdentCase::Snake { "_" } else { "-" };
            words
                .iter()
                .map(|w| w.to_ascii_lowercase())
                .collect::<Vec<_>>()
                .join(sep)
        }
    };
    format!("{lead}{body}{trail}")
}

/// 文字列の中の識別子（英数字・`_`・`-` の並び）をそれぞれ変換する。
/// 行が英数字・`_`・`-`・空白だけなら、空白も単語の区切りとして 1 つの識別子にする
/// （`hello world` → `helloWorld`）。
fn convert_identifiers(text: &str, case: IdentCase) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let content = line.trim_end_matches(['\n', '\r']);
        let eol = &line[content.len()..];
        let trimmed = content.trim();
        if !trimmed.is_empty()
            && trimmed.contains(' ')
            && trimmed.chars().all(|c| is_ident_char(c) || c == ' ')
        {
            let start = content.find(trimmed).unwrap_or(0);
            out.push_str(&content[..start]);
            out.push_str(&convert_ident(trimmed, case));
            out.push_str(&content[start + trimmed.len()..]);
        } else {
            let mut token = String::new();
            for c in content.chars() {
                if is_ident_char(c) {
                    token.push(c);
                } else {
                    if !token.is_empty() {
                        out.push_str(&convert_ident(&token, case));
                        token.clear();
                    }
                    out.push(c);
                }
            }
            if !token.is_empty() {
                out.push_str(&convert_ident(&token, case));
            }
        }
        out.push_str(eol);
    }
    out
}

// ---- 行の重複の除去 --------------------------------------------------------------

/// 行の比較に使う値（128 ビットのハッシュ。行の内容は保持しない）。
fn line_key(line: &[u8]) -> (u64, u64) {
    let mut a = std::collections::hash_map::DefaultHasher::new();
    line.hash(&mut a);
    let mut b = std::collections::hash_map::DefaultHasher::new();
    0x9E37_79B9_7F4A_7C15u64.hash(&mut b);
    line.hash(&mut b);
    (a.finish(), b.finish())
}

/// `snap` の範囲 `range`（行の境界にそろえたもの）の中で、前に同じ内容の行があれば
/// その行を除いた文書全体を `w` に書く。行の比較では改行コードを無視する。
/// 除いた行の数を返す。`step(処理した位置)` が `false` を返したら中止する。
pub(crate) fn dedup_lines(
    snap: &Snapshot,
    range: std::ops::Range<u64>,
    w: &mut dyn Write,
    step: &mut dyn FnMut(u64) -> bool,
) -> Result<u64, ReplaceError> {
    for c in snap.chunks(0..range.start) {
        w.write_all(c)?;
    }
    let mut seen: HashSet<(u64, u64)> = HashSet::new();
    let mut removed = 0u64;
    let mut pos = range.start;
    let mut line: Vec<u8> = Vec::new();
    let mut last_step = pos;
    while pos < range.end {
        let end = snap
            .find_next(pos..range.end, b'\n')
            .map_or(range.end, |n| n + 1);
        line.clear();
        for c in snap.chunks(pos..end) {
            line.extend_from_slice(c);
        }
        let content = line
            .strip_suffix(b"\n")
            .map(|l| l.strip_suffix(b"\r").unwrap_or(l))
            .unwrap_or(&line);
        if seen.insert(line_key(content)) {
            w.write_all(&line)?;
        } else {
            removed += 1;
        }
        pos = end;
        if pos - last_step >= 1 << 20 {
            last_step = pos;
            if !step(pos) {
                return Err(ReplaceError::Cancelled);
            }
        }
    }
    for c in snap.chunks(range.end..snap.len()) {
        w.write_all(c)?;
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upper_and_lower_only_touch_latin_letters() {
        assert_eq!(
            Transform::Upper.apply("abc ａｂｃ éß 日本 αβ"),
            "ABC ＡＢＣ ÉSS 日本 αβ"
        );
        assert_eq!(
            Transform::Lower.apply("ABC ＡＢＣ É 日本 ΑΒ"),
            "abc ａｂｃ é 日本 ΑΒ"
        );
    }

    #[test]
    fn katakana_width_conversion_round_trips() {
        let full = "ガギグゲゴパピプペポヴ・ーアイウエオヲンッャ、「日本」";
        let half = Transform::HalfKatakana.apply(full);
        assert_eq!(half, "ｶﾞｷﾞｸﾞｹﾞｺﾞﾊﾟﾋﾟﾌﾟﾍﾟﾎﾟｳﾞ･ｰｱｲｳｴｵｦﾝｯｬ、「日本」");
        assert_eq!(
            Transform::FullKatakana.apply(&half),
            "ガギグゲゴパピプペポヴ・ーアイウエオヲンッャ、「日本」"
        );
        // 全角の句読点・かぎかっこは半角にしないが、半角のものは全角にする
        assert_eq!(
            Transform::FullKatakana.apply("｢ｶﾀｶﾅ｣､ﾃﾞｽ｡"),
            "「カタカナ」、デス。"
        );
        // 半角にない文字・ひらがなはそのまま、濁点の付かない文字の濁点は単独の濁点に
        assert_eq!(Transform::HalfKatakana.apply("ヰヱがな"), "ヰヱがな");
        assert_eq!(Transform::FullKatakana.apply("ｱﾞﾏﾟ"), "ア゛マ゜");
        assert_eq!(
            Transform::FullKatakana.apply("ﾀﾞﾁﾞﾂﾞﾃﾞﾄﾞﾊﾞﾋﾟﾜﾞｦﾞ"),
            "ダヂヅデドバピヷヺ"
        );
    }

    #[test]
    fn identifier_case_conversion() {
        assert_eq!(Transform::Camel.apply("hello_world"), "helloWorld");
        assert_eq!(Transform::Camel.apply("Hello-World-2"), "helloWorld2");
        assert_eq!(Transform::Camel.apply("HTTPServer"), "httpServer");
        assert_eq!(
            Transform::Snake.apply("parseHTTPResponse"),
            "parse_http_response"
        );
        assert_eq!(Transform::Snake.apply("utf8Decoder"), "utf8_decoder");
        assert_eq!(
            Transform::Kebab.apply("myVariable_name"),
            "my-variable-name"
        );
        // 前後の `_` は残す
        assert_eq!(Transform::Camel.apply("__private_value"), "__privateValue");
        // 空白で区切った単語の並び
        assert_eq!(Transform::Camel.apply("  hello world\n"), "  helloWorld\n");
        assert_eq!(Transform::Kebab.apply("Hello World"), "hello-world");
        // 記号を含む行は識別子ごとに変換する
        assert_eq!(
            Transform::Snake.apply("let fooBar = bazQux(x);"),
            "let foo_bar = baz_qux(x);"
        );
        assert_eq!(Transform::Snake.apply("日本語 fooBar"), "日本語 foo_bar");
    }

    #[test]
    fn dedups_lines_in_range() {
        let text = "keep\nb\na\nb\r\nc\na\nb";
        let snap = Snapshot::from_bytes(text);
        let mut out = Vec::new();
        let n = dedup_lines(&snap, 5..snap.len(), &mut out, &mut |_| true).unwrap();
        assert_eq!(n, 3);
        assert_eq!(String::from_utf8(out).unwrap(), "keep\nb\na\nc\n");
        // 範囲の外の行は比べない
        let snap = Snapshot::from_bytes("x\ny\nx\ny\n");
        let mut out = Vec::new();
        assert_eq!(
            dedup_lines(&snap, 4..8, &mut out, &mut |_| true).unwrap(),
            0
        );
        assert_eq!(out, b"x\ny\nx\ny\n");
    }
}
