use crate::{Output, Params, Result, checkpoint, integer, value};
use std::collections::HashSet;

pub(crate) trait Random {
    fn word(&mut self) -> Result<u64>;
    fn index(&mut self, len: usize) -> Result<usize> {
        if len == 0 {
            return Err("選択候補が空です".into());
        }
        let n = len as u64;
        let limit = u64::MAX - u64::MAX % n;
        loop {
            let word = self.word()?;
            if word < limit {
                return Ok((word % n) as usize);
            }
        }
    }
}

pub(crate) struct Secure {
    bytes: [u8; 4096],
    at: usize,
}
impl Default for Secure {
    fn default() -> Self {
        Self {
            bytes: [0; 4096],
            at: 4096,
        }
    }
}
impl Random for Secure {
    fn word(&mut self) -> Result<u64> {
        if self.at + 8 > self.bytes.len() {
            getrandom::fill(&mut self.bytes).map_err(|e| format!("OS乱数を取得できません: {e}"))?;
            self.at = 0;
        }
        let mut bytes = [0; 8];
        bytes.copy_from_slice(&self.bytes[self.at..self.at + 8]);
        self.at += 8;
        Ok(u64::from_le_bytes(bytes))
    }
}

/// SplitMix64 is for reproducible dummy data only, never for passwords.
pub(crate) struct Seeded(pub u64);
impl Random for Seeded {
    fn word(&mut self) -> Result<u64> {
        self.0 = self.0.wrapping_add(0x9e3779b97f4a7c15);
        let mut x = self.0;
        x = (x ^ (x >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        x = (x ^ (x >> 27)).wrapping_mul(0x94d049bb133111eb);
        Ok(x ^ (x >> 31))
    }
}

pub(crate) fn passwords(params: &Params, cancelled: &impl Fn() -> bool) -> Result<Output> {
    generate_passwords(params, cancelled, &mut Secure::default())
}

fn generate_passwords(
    params: &Params,
    cancelled: &impl Fn() -> bool,
    rng: &mut impl Random,
) -> Result<Output> {
    let length = integer(params, "length", 4, 256)? as usize;
    let count = integer(params, "count", 1, 1000)? as usize;
    let requirements = [
        integer(params, "lower", 0, 256)?,
        integer(params, "upper", 0, 256)?,
        integer(params, "digits", 0, 256)?,
        integer(params, "symbols_min", 0, 256)?,
    ];
    if requirements.iter().sum::<u32>() as usize > length {
        return Err("各文字種の最低数の合計が文字数を超えています".into());
    }
    let symbols = value(params, "symbols", "");
    if !symbols.chars().all(|c| c.is_ascii_punctuation()) {
        return Err("記号にはASCIIの記号のみ指定してください".into());
    }
    let exclude = value(params, "exclude", "");
    let alphabets = [
        "abcdefghijklmnopqrstuvwxyz",
        "ABCDEFGHIJKLMNOPQRSTUVWXYZ",
        "0123456789",
        symbols,
    ];
    let mut groups = Vec::new();
    let mut all = Vec::new();
    for (alphabet, &min) in alphabets.iter().zip(&requirements) {
        if min == 0 {
            continue;
        }
        let mut chars = Vec::new();
        for c in alphabet.chars().filter(|c| !exclude.contains(*c)) {
            if !chars.contains(&c) {
                chars.push(c);
            }
        }
        if chars.is_empty() {
            return Err("除外後の文字種が空です。最低数・記号・除外文字を確認してください".into());
        }
        all.extend(chars.iter().copied());
        groups.push((chars, min as usize));
    }
    if all.is_empty() {
        return Err("使用する文字種を少なくとも1つ指定してください".into());
    }
    if all
        .len()
        .checked_pow(length as u32)
        .is_some_and(|space| space < count)
    {
        return Err("条件を満たす異なる候補が足りません".into());
    }
    let mut unique = HashSet::new();
    let mut rows = Vec::new();
    for _ in 0..count * 100 + 100 {
        checkpoint(cancelled)?;
        let mut chars = Vec::new();
        for (group, min) in &groups {
            for _ in 0..*min {
                chars.push(group[rng.index(group.len())?]);
            }
        }
        while chars.len() < length {
            chars.push(all[rng.index(all.len())?]);
        }
        for i in (1..chars.len()).rev() {
            let j = rng.index(i + 1)?;
            chars.swap(i, j);
        }
        let password: String = chars.into_iter().collect();
        if unique.insert(password.clone()) {
            rows.push(password);
        }
        if rows.len() == count {
            return Ok(Output::text(
                rows.join("\n"),
                "password-candidates.txt",
                format!("OS乱数で異なる候補を{count}件生成しました。入力・結果は履歴保存しません"),
            ));
        }
    }
    Err("条件を満たす異なる候補を指定数生成できません。文字数・文字種を増やすか、件数を減らしてください".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_password_meets_minima_and_exclusions() {
        let mut p = crate::Tool::Password.defaults();
        p.insert("count".into(), "500".into());
        p.insert("digits".into(), "4".into());
        let result = generate_passwords(&p, &|| false, &mut Seeded(1)).unwrap();
        let rows: Vec<_> = result.text.lines().collect();
        assert_eq!(rows.len(), 500);
        assert_eq!(rows.iter().collect::<HashSet<_>>().len(), 500);
        for row in rows {
            assert_eq!(row.len(), 20);
            assert!(row.chars().filter(char::is_ascii_digit).count() >= 4);
            assert!(row.chars().any(|c| c.is_ascii_lowercase()));
            assert!(row.chars().any(|c| c.is_ascii_uppercase()));
            assert!(row.chars().any(|c| c.is_ascii_punctuation()));
            assert!(!row.contains(['I', 'l', '1', 'O', '0', 'o']));
        }
        p.insert("length".into(), "4".into());
        assert!(generate_passwords(&p, &|| false, &mut Seeded(1)).is_err());
        assert!(
            generate_passwords(&crate::Tool::Password.defaults(), &|| true, &mut Seeded(1))
                .is_err()
        );
    }
}
