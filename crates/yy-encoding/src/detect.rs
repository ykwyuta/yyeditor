//! 文字コードの自動判別（03 章 3）。

use crate::{Encoding, decode_all};

/// 判別結果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Detected {
    pub encoding: Encoding,
    /// 先頭の BOM のバイト数（BOM がなければ 0）
    pub bom_len: usize,
}

/// ファイルの先頭 `sample`（`complete` ならファイル全体）から文字コードを推定する。
///
/// 1. BOM
/// 2. BOM なしの UTF-16 / UTF-32（0x00 の出現位置の偏り）
/// 3. 8 ビットのバイトがなく ISO-2022-JP のエスケープシーケンスがある → ISO-2022-JP
/// 4. UTF-8 として正しい（ASCII のみを含む）→ UTF-8
/// 5. CP932 と EUC-JP の不正バイト数・文字種で比較
/// 6. どちらにも不正なバイトが多ければ chardetng の推定
pub fn detect(sample: &[u8], complete: bool) -> Detected {
    for enc in [
        Encoding::Utf32Le,
        Encoding::Utf32Be,
        Encoding::Utf8,
        Encoding::Utf16Le,
        Encoding::Utf16Be,
    ] {
        let bom = enc.bom();
        if sample.starts_with(bom) {
            // "FF FE 00 00" は UTF-16LE の BOM + U+0000 の可能性もある
            if enc == Encoding::Utf32Le && complete && sample.len() % 4 != 0 {
                continue;
            }
            return Detected {
                encoding: enc,
                bom_len: bom.len(),
            };
        }
    }
    let encoding = detect_without_bom(sample, complete);
    Detected {
        encoding,
        bom_len: 0,
    }
}

fn detect_without_bom(sample: &[u8], complete: bool) -> Encoding {
    if let Some(e) = detect_wide(sample) {
        return e;
    }
    let high = sample.iter().any(|&b| b >= 0x80);
    if !high {
        if sample.contains(&0x1B) && has_iso2022_escape(sample) {
            return Encoding::Iso2022Jp;
        }
        return Encoding::Utf8;
    }
    if is_utf8(sample, complete) {
        return Encoding::Utf8;
    }
    if let Some(e) = detect_utf16_text(sample) {
        return e;
    }
    // 末尾で切れた文字は数えない
    let body = if complete {
        sample
    } else {
        &sample[..sample.len().saturating_sub(3)]
    };
    let sjis = japanese_score(Encoding::Cp932, body);
    let euc = japanese_score(Encoding::EucJp, body);
    let nonascii = body.iter().filter(|&&b| b >= 0x80).count().max(1) as u64;
    // 不正なバイトが少ない（非 ASCII の 5% 以下）か、日本語らしさの点数が不正バイトより十分に多い
    let ok = |s: &Score| s.invalid * 20 <= nonascii || s.points >= 8 * s.invalid as i64;
    match (ok(&sjis), ok(&euc)) {
        (true, false) => Encoding::Cp932,
        (false, true) => Encoding::EucJp,
        (true, true) => {
            if euc.points > sjis.points {
                Encoding::EucJp
            } else {
                Encoding::Cp932
            }
        }
        (false, false) => {
            let mut d = chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Allow);
            d.feed(body, complete);
            Encoding::from_web(d.guess(None, chardetng::Utf8Detection::Allow))
        }
    }
}

/// UTF-8 として正しいか（`complete` でなければ末尾の途中で切れた文字は許す）。
fn is_utf8(s: &[u8], complete: bool) -> bool {
    match std::str::from_utf8(s) {
        Ok(_) => true,
        Err(e) => !complete && e.error_len().is_none() && s.len() - e.valid_up_to() < 4,
    }
}

fn has_iso2022_escape(s: &[u8]) -> bool {
    const SEQS: [&[u8]; 7] = [
        b"\x1B$B", b"\x1B$@", b"\x1B(J", b"\x1B(I", b"\x1B$(D", b"\x1B$(O", b"\x1B$(Q",
    ];
    s.windows(3)
        .enumerate()
        .filter(|(_, w)| w[0] == 0x1B)
        .any(|(i, _)| SEQS.iter().any(|q| s[i..].starts_with(q)))
}

/// BOM なしの UTF-16 / UTF-32。ASCII が大半のテキストでは 0x00 が決まった位置に並ぶ。
fn detect_wide(s: &[u8]) -> Option<Encoding> {
    if s.len() < 4 {
        return None;
    }
    let mut zeros = [0usize; 4];
    for (i, &b) in s.iter().enumerate() {
        if b == 0 {
            zeros[i % 4] += 1;
        }
    }
    let quarter = s.len() / 4;
    let many = |n: usize| n * 10 >= quarter * 7;
    let few = |n: usize| n * 20 <= quarter;
    if many(zeros[1]) && many(zeros[2]) && many(zeros[3]) && few(zeros[0]) {
        return Some(Encoding::Utf32Le);
    }
    if many(zeros[0]) && many(zeros[1]) && many(zeros[2]) && few(zeros[3]) {
        return Some(Encoding::Utf32Be);
    }
    let even = zeros[0] + zeros[2];
    let odd = zeros[1] + zeros[3];
    let half = s.len() / 2;
    let many2 = |n: usize| n * 10 >= half * 4;
    let few2 = |n: usize| n * 20 <= half;
    if many2(odd) && few2(even) {
        return Some(Encoding::Utf16Le);
    }
    if many2(even) && few2(odd) {
        return Some(Encoding::Utf16Be);
    }
    None
}

/// 日本語の多い BOM なし UTF-16 は 0x00 がほとんど現れないので、デコード結果の文字種で判定する。
/// かなを含み、ほぼすべてがかな・漢字・全角・ASCII の文字ならその UTF-16 とする。
fn detect_utf16_text(s: &[u8]) -> Option<Encoding> {
    let body = &s[..s.len() & !1];
    if body.len() < 4 {
        return None;
    }
    [Encoding::Utf16Le, Encoding::Utf16Be]
        .into_iter()
        .find(|&enc| {
            let (text, stats) = decode_all(enc, body, false);
            if stats.invalid > 0 {
                return false;
            }
            let text = String::from_utf8_lossy(&text);
            let (mut total, mut plausible, mut kana) = (0usize, 0usize, 0usize);
            for c in text.chars() {
                total += 1;
                match c as u32 {
                    0x3040..=0x30FF => {
                        kana += 1;
                        plausible += 1;
                    }
                    0x09
                    | 0x0A
                    | 0x0D
                    | 0x20..=0x7E
                    | 0x3000..=0x303F
                    | 0x4E00..=0x9FFF
                    | 0xFF00..=0xFFEF => plausible += 1,
                    _ => {}
                }
            }
            kana > 0 && plausible * 100 >= total * 95
        })
}

struct Score {
    invalid: u64,
    points: i64,
}

/// デコード結果の文字種で日本語らしさを点数にする。
/// EUC-JP を CP932 として読むと半角カナが、CP932 を EUC-JP として読むと不正バイトが多くなる。
fn japanese_score(enc: Encoding, s: &[u8]) -> Score {
    let (text, stats) = decode_all(enc, s, false);
    let text = String::from_utf8_lossy(&text);
    let mut points = 0i64;
    for c in text.chars() {
        points += match c as u32 {
            0x3041..=0x309F => 3,  // ひらがな
            0x30A0..=0x30FF => 2,  // カタカナ
            0x3000..=0x303F => 2,  // 句読点など
            0x4E00..=0x9FFF => 1,  // 漢字
            0xFF01..=0xFF5E => 1,  // 全角英数
            0xFF61..=0xFF9F => -2, // 半角カナ
            0xE000..=0xF8FF => -4, // 私用領域（外字）
            0xFFFD => -8,
            _ => 0,
        };
    }
    Score {
        invalid: stats.invalid,
        points,
    }
}

/// EBCDIC らしければその文字コードを推定する（03 章 3.3）。既定の自動判別には含めず、
/// 設定で有効にした場合だけ使う。`file_len` はファイル全体の大きさ（固定長レコードの推定用）。
///
/// EBCDIC の空白（0x40）が ASCII の空白（0x20）より十分に多く、SO…SI の外がほぼすべて
/// EBCDIC の英数字・記号・空白ならその文字コードとする。
pub fn detect_ebcdic(sample: &[u8], file_len: u64) -> Option<Encoding> {
    use crate::{Ccsid, Records};
    if sample.len() < 16 {
        return None;
    }
    let (mut single, mut plausible, mut spaces, mut shifts) = (0usize, 0usize, 0usize, 0usize);
    let mut double = false;
    for &b in sample {
        match b {
            0x0E if !double => {
                double = true;
                shifts += 1;
                continue;
            }
            0x0F if double => {
                double = false;
                continue;
            }
            _ if double => continue,
            _ => {}
        }
        single += 1;
        if b == 0x40 {
            spaces += 1;
        }
        // 英小文字系（939）・カタカナ系（930）のどちらかで表示できる文字か、改行・タブ
        if matches!(b, 0x05 | 0x0D | 0x15 | 0x25)
            || Ccsid::Ibm939.table().is_printable(b)
            || Ccsid::Ibm930.table().is_printable(b)
        {
            plausible += 1;
        }
    }
    let ascii_spaces = sample.iter().filter(|&&b| b == 0x20).count();
    if single == 0
        || plausible * 100 < single * 90
        || spaces * 100 < single * 3
        || spaces < ascii_spaces * 4
    {
        return None;
    }
    let records = if sample.contains(&0x15) {
        Records::Nl
    } else if sample.contains(&0x25) {
        Records::Lf
    } else {
        const LENGTHS: [u64; 9] = [80, 72, 120, 128, 132, 133, 256, 512, 1024];
        Records::Fixed(
            LENGTHS
                .into_iter()
                .find(|n| file_len % n == 0)
                .unwrap_or(80) as u32,
        )
    };
    let candidates: &[Ccsid] = if shifts > 0 {
        &[Ccsid::Ibm939, Ccsid::Ibm930, Ccsid::Ibm1399, Ccsid::Ibm1390]
    } else {
        &[
            Ccsid::Ibm037,
            Ccsid::Ibm1047,
            Ccsid::Ibm500,
            Ccsid::Ibm1027,
            Ccsid::Ibm290,
        ]
    };
    // 不正なバイトが最も少ないもの。同じなら英小文字・半角カナの並びが自然なもの
    // （930 と 939 は英小文字と半角カナの位置が入れ替わっている）、それも同じなら先に挙げたもの
    let best = candidates
        .iter()
        .map(|&c| {
            let enc = Encoding::Ebcdic(c, records);
            let (text, stats) = decode_all(enc, sample, true);
            let points = kana_latin_score(&String::from_utf8_lossy(&text));
            ((stats.invalid, std::cmp::Reverse(points)), enc)
        })
        .min_by_key(|(score, _)| *score)?;
    Some(best.1)
}

/// 英小文字・半角カナの並びの自然さ。濁点・半濁点（どちらの表でも同じ位置）がカナ以外の後に
/// 来たり、英小文字とカナが隣り合ったりするのは、別の表で読んでいるしるし。
fn kana_latin_score(text: &str) -> i64 {
    #[derive(PartialEq)]
    enum Class {
        Lower,
        Kana,
        Other,
    }
    let mut points = 0i64;
    let mut prev = Class::Other;
    for c in text.chars() {
        let class = match c {
            'a'..='z' => Class::Lower,
            '\u{FF66}'..='\u{FF9D}' => Class::Kana,
            _ => Class::Other,
        };
        points += match (&class, &prev, c) {
            (Class::Lower, Class::Kana, _) | (Class::Kana, Class::Lower, _) => -3,
            (Class::Lower, _, _) => 2,
            (Class::Kana, _, _) => 1,
            (_, Class::Kana, '\u{FF9E}' | '\u{FF9F}') => 1,
            (_, _, '\u{FF9E}' | '\u{FF9F}') => -5,
            _ => 0,
        };
        // 濁点はカナの一部として扱う
        if !matches!(c, '\u{FF9E}' | '\u{FF9F}') || prev != Class::Kana {
            prev = class;
        }
    }
    points
}
