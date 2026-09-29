//! 保存時の文字の正規化（03 章 4.1）。
//!
//! 保存先の文字コードで表せない文字を、見た目・意味の近い文字に置き換える。
//! 互換文字の畳み込み（① → (1)、Ⅱ → II、㈱ → (株)）、半角カナ → 全角カナ（濁点は合成）など。

use unicode_normalization::UnicodeNormalization;

use crate::{Encoding, EscapeMode, encode_all};

/// 保存できない文字列 `s` の代わりに書ける文字列。置き換えても保存できなければ `None`。
pub fn fold_compat(enc: Encoding, s: &str) -> Option<String> {
    let mut pre = String::with_capacity(s.len());
    for c in s.chars() {
        match c as u32 {
            // 丸数字は NFKC では数字だけになるので括弧を付ける（JIS の慣習）
            0x2460..=0x2473 => pre += &format!("({})", c as u32 - 0x2460 + 1),
            0x24EA => pre += "(0)",
            0x2776..=0x277F => pre += &format!("({})", c as u32 - 0x2776 + 1),
            _ => pre.push(c),
        }
    }
    let folded: String = pre.nfkc().collect();
    if folded == s || folded.is_empty() {
        return None;
    }
    encode_all(enc, folded.as_bytes(), EscapeMode::Reject)
        .ok()
        .map(|_| folded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_compatibility_characters() {
        let sjis = Encoding::ShiftJis;
        assert_eq!(fold_compat(sjis, "①").as_deref(), Some("(1)"));
        assert_eq!(fold_compat(sjis, "⑳").as_deref(), Some("(20)"));
        assert_eq!(fold_compat(sjis, "Ⅱ").as_deref(), Some("II"));
        assert_eq!(fold_compat(sjis, "㈱").as_deref(), Some("(株)"));
        // 半角カナは全角に、濁点は合成する
        assert_eq!(fold_compat(sjis, "ｶﾞ").as_deref(), Some("ガ"));
        // 全角英数は半角に
        let latin = Encoding::from_name("windows-1252").unwrap();
        assert_eq!(fold_compat(latin, "ＡＢＣ").as_deref(), Some("ABC"));
        let ebcdic = Encoding::from_name("IBM-037").unwrap();
        assert_eq!(fold_compat(ebcdic, "１２").as_deref(), Some("12"));
        // 置き換えても保存できない・置き換える文字がない
        assert_eq!(fold_compat(latin, "漢"), None);
        assert_eq!(fold_compat(ebcdic, "ｶﾞ"), None);
        assert_eq!(fold_compat(sjis, "A"), None);
    }
}
