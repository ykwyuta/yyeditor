//! レコード長の候補と、編集用のコピーブックを見本から作る。

use std::collections::BTreeMap;

use yy_cobol::{Charset, Codec};

use super::RecordSep;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Text,
    Digit,
    Space,
}

/// レコード長の候補。一致度は統計上の確率ではなく、候補を並べるための目安。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordLengthGuess {
    pub data_len: usize,
    pub separator: RecordSep,
    pub score: u16,
}

/// 区切りの周期、またはファイルサイズの約数とバイト・文字種の繰り返しから長さの候補を返す。
/// 区切りなしは 65536 バイトまで、見本中に最低 2 件ある長さを調べる。マルチレイアウトで交互に
/// 現れる形も拾うため 2 件おきの一致も見る。候補は確定値ではなく、利用者が確認・修正して使う。
pub fn infer_record_lengths(
    sample: &[u8],
    file_len: u64,
    charset: Charset,
) -> Vec<RecordLengthGuess> {
    let mut guesses = Vec::new();
    for separator in [RecordSep::Crlf, RecordSep::Lf, RecordSep::Nl] {
        if separator == RecordSep::Nl && !charset.is_ebcdic() {
            continue;
        }
        let sep = separator.bytes();
        let Some(len) = sample.windows(sep.len()).position(|w| w == sep) else {
            continue;
        };
        if len == 0 || len > (sample.len() - sep.len()) / 2 {
            continue;
        }
        let stride = len + sep.len();
        let (records, rest) = super::record_count(file_len, len, separator);
        if records < 2 || rest != 0 {
            continue;
        }
        let valid = (0..sample.len() / stride).take(4096).all(|k| {
            let start = k * stride;
            &sample[start + len..start + stride] == sep
                && !sample[start..start + len]
                    .windows(sep.len())
                    .any(|w| w == sep)
        });
        if valid {
            guesses.push(RecordLengthGuess {
                data_len: len,
                separator,
                score: 1000,
            });
        }
    }
    // 区切りが規則的に存在するなら、それをデータとして扱う約数候補は出さない。
    if !guesses.is_empty() {
        return guesses;
    }
    let sample = &sample[..sample.len().min(1 << 20)];
    let classes = classes(sample, charset);
    let mut byte_counts = [0usize; 256];
    let mut class_counts = [0usize; 3];
    for (&b, &c) in sample.iter().zip(&classes) {
        byte_counts[b as usize] += 1;
        class_counts[c as usize] += 1;
    }
    let baseline = |counts: &[usize]| {
        let n = sample.len().max(1) as f64;
        counts.iter().map(|&c| (c as f64 / n).powi(2)).sum::<f64>()
    };
    let byte_base = baseline(&byte_counts);
    let class_base = baseline(&class_counts);
    for len in 1..=65536.min(sample.len() / 2) {
        if file_len % len as u64 != 0 {
            continue;
        }
        let n = (sample.len() / len).min(128).min((65536 / len).max(2));
        let mut best = 0.0f64;
        for lag in [1, 2] {
            if n <= lag || lag == 2 && n < 4 {
                continue;
            }
            let offset = lag * len;
            let count = (n - lag) * len;
            let mut bytes_equal = 0usize;
            let mut classes_equal = 0usize;
            for i in 0..count {
                bytes_equal += usize::from(sample[i] == sample[i + offset]);
                classes_equal += usize::from(classes[i] == classes[i + offset]);
            }
            let gain = |equal: usize, base: f64| {
                if base >= 1.0 {
                    0.0
                } else {
                    ((equal as f64 / count as f64 - base) / (1.0 - base)).max(0.0)
                }
            };
            let score = (0.7 * gain(classes_equal, class_base)
                + 0.3 * gain(bytes_equal, byte_base))
                * (n - lag) as f64
                / ((n - lag) as f64 + 4.0);
            best = best.max(score);
        }
        guesses.push(RecordLengthGuess {
            data_len: len,
            separator: RecordSep::None,
            score: (best * 1000.0) as u16,
        });
    }
    guesses.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.data_len.cmp(&b.data_len))
    });
    guesses.truncate(12);
    guesses
}

// 2 バイト文字の途中に、数字・空白に見えるバイトがあっても分割しない。
fn classes(record: &[u8], charset: Charset) -> Vec<Class> {
    let mut out = vec![Class::Text; record.len()];
    let mut i = 0;
    let mut dbcs = false;
    while i < record.len() {
        let b = record[i];
        if charset.is_ebcdic() {
            if b == 0x0e {
                dbcs = true;
            } else if b == 0x0f {
                dbcs = false;
            } else if dbcs {
                i += 2;
                continue;
            } else if (0xf0..=0xf9).contains(&b) {
                out[i] = Class::Digit;
            } else if b == charset.space() {
                out[i] = Class::Space;
            }
        } else if (0x81..=0x9f).contains(&b) || (0xe0..=0xfc).contains(&b) {
            i += 2;
            continue;
        } else if b.is_ascii_digit() {
            out[i] = Class::Digit;
        } else if b == charset.space() {
            out[i] = Class::Space;
        }
        i += 1;
    }
    out
}

/// 先頭の見本（最大 4096 レコード）からレイアウトを推定する。
///
/// `data_len` は利用者が指定するバイト数（区切りを含まない）。末尾の不完全なレコードは使わない。
/// マルチでは数字の位置が一致するレコードをまとめ、グループごとに推定する。単一では全レコードに
/// 共通する数字・空白だけを使う。連続する文字・数字の中の業務上の項目境界、符号・小数点・COMP 系の
/// 型は推定しない。結果は通常のコピーブックなので、利用者が項目名・境界・型を編集できる。
/// 戻り値は（名前, コピーブック）。数字の多い候補を先に命名し、既存の名前順の割り当てで優先する。
pub fn infer_layouts(
    sample: &[u8],
    data_len: usize,
    codec: Codec,
    separator: RecordSep,
    multi: bool,
) -> Result<Vec<(String, String)>, String> {
    if data_len == 0 {
        return Err("推定する前にレコード長（バイト）を入力してください".into());
    }
    let stride = data_len
        .checked_add(separator.bytes().len())
        .ok_or("レコード長が大きすぎます")?;
    if sample.len() < data_len {
        return Err(
            "見本に完全なレコードがありません。レコード長を確認してください（見本は先頭 1 MB）"
                .into(),
        );
    }
    let mut groups: BTreeMap<Vec<bool>, Vec<Class>> = BTreeMap::new();
    for at in (0..=sample.len() - data_len).step_by(stride).take(4096) {
        let end = at + data_len;
        let sep = separator.bytes();
        if !sep.is_empty() && end < sample.len() {
            let remaining = &sample[end..];
            // 見本の末尾で区切りが途中までしかない場合は許容する。
            if !remaining.starts_with(sep) && !sep.starts_with(remaining) {
                return Err("指定したレコード長・区切りでレコードを区切れません".into());
            }
        }
        let row = classes(&sample[at..end], codec.charset);
        let key = if multi {
            row.iter().map(|c| *c == Class::Digit).collect()
        } else {
            Vec::new()
        };
        if let Some(common) = groups.get_mut(&key) {
            for (a, b) in common.iter_mut().zip(row) {
                if *a != b {
                    *a = Class::Text;
                }
            }
        } else {
            if groups.len() >= 64 {
                return Err("候補が 64 種を超えました。レコード長・文字コードを確認するか、手動でレイアウトを設定してください".into());
            }
            groups.insert(key, row);
        }
    }
    let mut groups: Vec<_> = groups.into_iter().collect();
    groups.sort_by(|(a, _), (b, _)| {
        b.iter()
            .filter(|v| **v)
            .count()
            .cmp(&a.iter().filter(|v| **v).count())
            .then_with(|| a.cmp(b))
    });
    groups
        .into_iter()
        .enumerate()
        .map(|(k, (_, row))| {
            let name = format!("LAYOUT{:02}", k + 1);
            let mut copybook = format!("       01 {name}.\n");
            let mut start = 0;
            let mut field = 0;
            while start < row.len() {
                let class = row[start];
                let mut end = start + 1;
                while end < row.len() && row[end] == class {
                    end += 1;
                }
                if end - start > 1_000_000 {
                    return Err("連続する項目が PIC の上限（1000000 バイト）を超えています。手動で項目を分割してください".into());
                }
                field += 1;
                if field > 512 {
                    return Err(
                        "項目の候補が 512 個を超えました。レコード長・文字コードを確認してください"
                            .into(),
                    );
                }
                // COBOL の数値は 38 桁まで。長い数字の連続も切らずに文字列として残す。
                let pic = if class == Class::Digit && end - start <= 38 {
                    '9'
                } else {
                    'X'
                };
                copybook.push_str(&format!(
                    "           05 FIELD-{field:03} PIC {pic}({}).\n",
                    end - start
                ));
                start = end;
            }
            Ok((name, copybook))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixed::{FixedSpec, default_layout, layouts_by_name};

    fn codec() -> Codec {
        Codec {
            charset: Charset::Ms932,
            little_endian: false,
        }
    }

    #[test]
    fn single_uses_common_boundaries_and_ignores_partial_record() {
        let sample = b"001ALICE  \r\n002BOB    \r\n003CA";
        let inferred = infer_layouts(sample, 10, codec(), RecordSep::Crlf, false).unwrap();
        let spec = FixedSpec::new(&inferred[0].1, codec(), RecordSep::Crlf).unwrap();
        assert_eq!(spec.data_len, 10);
        assert_eq!(
            spec.layout.fields.iter().map(|f| f.len).collect::<Vec<_>>(),
            [3, 5, 2]
        );
        assert_eq!(spec.layout.fields[0].describe, "9(3)");
        // 推定後に項目名・境界・型を人が変更できる。
        let edited = inferred[0].1.replace(
            "FIELD-001 PIC 9(3)",
            "CODE PIC X(1).\n           05 NUMBER PIC 9(2)",
        );
        let spec = FixedSpec::new(&edited, codec(), RecordSep::Crlf).unwrap();
        assert_eq!(spec.layout.fields[0].name, "CODE");
        assert_eq!(spec.data_len, 10);
    }

    #[test]
    fn multi_groups_records_and_assigns_specific_layout_first() {
        let sample = b"AB123 XYCD456 ZZTEXTONLY";
        let inferred = infer_layouts(sample, 8, codec(), RecordSep::None, true).unwrap();
        assert_eq!(inferred.len(), 2);
        let spec = FixedSpec::new_multi(&inferred, 8, codec(), RecordSep::None).unwrap();
        let sorted = layouts_by_name(&spec);
        assert_eq!(
            default_layout(&spec, &sorted, &sample[..8])
                .unwrap()
                .name
                .as_ref(),
            "LAYOUT01"
        );
        assert_eq!(
            default_layout(&spec, &sorted, &sample[16..])
                .unwrap()
                .name
                .as_ref(),
            "LAYOUT02"
        );
        assert!(spec.multi.iter().all(|m| m.layout.record_len == 8));
    }

    #[test]
    fn dbcs_bytes_are_never_split_as_digits_or_spaces() {
        // MS932 の「０」と全角空白は、それぞれ 2 バイトのまま扱う。
        let inferred = infer_layouts(
            &[0x82, 0x4f, 0x81, 0x40, b'1'],
            5,
            codec(),
            RecordSep::None,
            false,
        )
        .unwrap();
        let layout = yy_cobol::parse(&inferred[0].1).unwrap();
        assert_eq!(
            layout.fields.iter().map(|f| f.len).collect::<Vec<_>>(),
            [4, 1]
        );
        let mut ebcdic = codec();
        ebcdic.charset = Charset::all().into_iter().find(|c| c.is_ebcdic()).unwrap();
        let inferred = infer_layouts(
            &[0x0e, 0xf1, 0x40, 0x0f, 0xf2, 0x40],
            6,
            ebcdic,
            RecordSep::None,
            false,
        )
        .unwrap();
        let layout = yy_cobol::parse(&inferred[0].1).unwrap();
        assert_eq!(
            layout.fields.iter().map(|f| f.len).collect::<Vec<_>>(),
            [4, 1, 1]
        );
    }

    #[test]
    fn requires_length_complete_record_and_valid_separator() {
        assert!(infer_layouts(b"ABC", 0, codec(), RecordSep::None, false).is_err());
        assert!(infer_layouts(b"ABC", 4, codec(), RecordSep::None, false).is_err());
        assert!(infer_layouts(b"ABC!DEF", 3, codec(), RecordSep::Lf, false).is_err());
        assert!(infer_layouts(b"ABC", usize::MAX, codec(), RecordSep::Lf, false).is_err());
    }

    #[test]
    fn length_candidates_detect_separators_with_optional_final_separator() {
        for (bytes, len, sep) in [
            (b"001ABC\r\n002DEF".as_slice(), 6, RecordSep::Crlf),
            (b"001ABC\n002DEF\n".as_slice(), 6, RecordSep::Lf),
            (b"ABC\x15DEF\x15".as_slice(), 3, RecordSep::Nl),
        ] {
            let charset = if sep == RecordSep::Nl {
                Charset::all().into_iter().find(|c| c.is_ebcdic()).unwrap()
            } else {
                Charset::Ms932
            };
            let guesses = infer_record_lengths(bytes, bytes.len() as u64, charset);
            assert_eq!(
                guesses[0],
                RecordLengthGuess {
                    data_len: len,
                    separator: sep,
                    score: 1000
                }
            );
        }
    }

    #[test]
    fn length_candidates_include_repeating_single_and_multi_records() {
        for multi in [false, true] {
            let mut bytes = Vec::new();
            for k in 0..60 {
                if multi && k % 2 == 0 {
                    bytes.extend_from_slice(b"HDR   DATA");
                } else {
                    bytes.extend_from_slice(format!("{k:03}NAME   ").as_bytes());
                }
            }
            let guesses = infer_record_lengths(&bytes, bytes.len() as u64, Charset::Ms932);
            // 半分・倍の長さも成立し得る。正しい長さを候補から人が選べることを確かめる。
            assert!(
                guesses.iter().any(|g| g.data_len == 10 && g.score > 0),
                "{guesses:?}"
            );
            assert_eq!(guesses[0].separator, RecordSep::None);
        }
        assert!(infer_record_lengths(b"", 0, Charset::Ms932).is_empty());
        assert!(infer_record_lengths(b"A", 1, Charset::Ms932).is_empty());
    }

    #[test]
    fn inferred_multi_layouts_import_export_and_persist() {
        use crate::{Context, Document, Value, fixed, yys};
        let ctx = Context::for_tests();
        let dir = tempfile::tempdir().unwrap();
        let bytes = b"H20261019   \r\nDABC00123   \r\nDXYZ00456   \r\n";
        let sep = RecordSep::Crlf;
        let inferred = infer_layouts(bytes, 12, codec(), sep, true).unwrap();
        let spec = FixedSpec::new_multi(&inferred, 12, codec(), sep).unwrap();
        assert_eq!(spec.multi.len(), 2);
        let input = dir.path().join("input.dat");
        std::fs::write(&input, bytes).unwrap();
        let (sheet, rep) = fixed::import_multi(&ctx, &input, &spec, &|_, _| true).unwrap();
        assert_eq!((rep.records, rep.invalid, rep.undetermined), (3, 0, 0));
        assert_eq!(sheet.get(&ctx, 1, 0).unwrap(), Value::text("LAYOUT01"));
        assert_eq!(sheet.get(&ctx, 2, 0).unwrap(), Value::text("LAYOUT02"));
        let output = dir.path().join("output.dat");
        fixed::export(&ctx, &sheet, &output, &spec, None, &|_, _| true).unwrap();
        assert_eq!(std::fs::read(output).unwrap(), bytes);
        let mut doc = Document::new(ctx.clone());
        doc.book.sheets = vec![sheet];
        let path = dir.path().join("inferred.yys");
        yys::save(&mut doc, &path, &mut |_, _| true).unwrap();
        let loaded = yys::open(ctx.clone(), &path).unwrap();
        assert_eq!(loaded.book.sheets[0].fixed.as_deref(), Some(&spec));
        assert_eq!(
            loaded.book.sheets[0].get(&ctx, 2, 0).unwrap(),
            Value::text("LAYOUT02")
        );
    }

    #[test]
    fn long_digit_runs_remain_editable_text() {
        let inferred = infer_layouts(&[b'1'; 80], 40, codec(), RecordSep::None, false).unwrap();
        let spec = FixedSpec::new(&inferred[0].1, codec(), RecordSep::None).unwrap();
        assert_eq!(spec.layout.fields.len(), 1);
        assert_eq!(spec.layout.fields[0].describe, "X(40)");
    }
}
