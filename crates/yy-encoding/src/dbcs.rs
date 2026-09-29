//! 表駆動の日本語マルチバイト文字コード（Shift_JIS 系・EUC-JP 系）。
//!
//! 対応表は初めて使うときに作る。CP932 と EUC-JP は `encoding_rs`（WHATWG の対応表）から、
//! JIS X 0208 準拠の Shift_JIS は CP932 の表から JIS の範囲だけを取り出して
//! 波ダッシュなどの 6 文字を JIS の対応に置き換えて、JIS X 0213 は Project X0213 の表から作る。

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::OnceLock;

use crate::tables::JISX0213_SJIS;
use crate::{Encoding, Sink};

/// 1 バイト目の種類（`single` の値。これ以外はそのバイトが表す文字）
const INVALID: u32 = u32::MAX;
const LEAD2: u32 = u32::MAX - 1;
const LEAD3: u32 = u32::MAX - 2;

/// デコード表の値のフラグ（0 は未定義）
const NONCANON: u32 = 1 << 30;
const PAIR: u32 = 1 << 29;
const CP_MASK: u32 = (1 << 29) - 1;

/// JIS と Microsoft で対応する Unicode が異なる文字（JIS の文字, MS の文字, CP932 の符号）。
/// 保存時はどちらの文字もその文字コードの符号に変換する（03 章 4.1）。
const JIS_MS: [(u32, u32, u16); 6] = [
    (0x301C, 0xFF5E, 0x8160), // 波ダッシュ / 全角チルダ
    (0x2016, 0x2225, 0x8161), // 双柱 / 平行記号
    (0x2212, 0xFF0D, 0x817C), // マイナス / 全角ハイフンマイナス
    (0x00A2, 0xFFE0, 0x8191), // セント
    (0x00A3, 0xFFE1, 0x8192), // ポンド
    (0x00AC, 0xFFE2, 0x81CA), // 否定
];

/// 対応表によって JIS X 0208 の 1 区 17 点・79 点に対応する Unicode が異なる文字（保存時の吸収のみ）。
const OVERLINE_YEN: [(u32, u32); 2] = [(0x203E, 0xFFE3), (0x00A5, 0xFFE5)];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Family {
    Sjis,
    Euc,
}

pub(crate) struct Table {
    family: Family,
    single: [u32; 256],
    /// 2 バイト符号（`上位 << 8 | 下位`）→ 文字
    dec2: Vec<u32>,
    /// EUC の 0x8F に続く 2 バイト → 文字
    dec3: Vec<u32>,
    /// 2 文字に対応する符号の文字の組
    pairs: Vec<(u32, u32)>,
    /// 文字 → 符号（`バイト数 << 24 | バイト列`、0 は変換不能）
    enc_bmp: Vec<u32>,
    enc_astral: HashMap<u32, u32>,
    enc_pairs: HashMap<(u32, u32), u32>,
    /// 組の 1 文字目になる文字（次の文字を見ないと符号が決まらない）
    bases: HashSet<u32>,
}

pub(crate) fn table(enc: Encoding) -> &'static Table {
    static SJIS: OnceLock<Table> = OnceLock::new();
    static CP932: OnceLock<Table> = OnceLock::new();
    static SJIS2004: OnceLock<Table> = OnceLock::new();
    static EUCJP: OnceLock<Table> = OnceLock::new();
    static EUC2004: OnceLock<Table> = OnceLock::new();
    match enc {
        Encoding::ShiftJis => SJIS.get_or_init(build_shift_jis),
        Encoding::Cp932 => CP932.get_or_init(build_cp932),
        Encoding::ShiftJis2004 => SJIS2004.get_or_init(|| build_2004(Family::Sjis)),
        Encoding::EucJp => EUCJP.get_or_init(build_euc_jp),
        Encoding::EucJis2004 => EUC2004.get_or_init(|| build_2004(Family::Euc)),
        _ => unreachable!("not a DBCS encoding"),
    }
}

fn pack(bytes: &[u8]) -> u32 {
    let mut v = (bytes.len() as u32) << 24;
    for &b in bytes {
        v = (v & 0xFF00_0000) | ((v & 0x00FF_FFFF) << 8) | b as u32;
    }
    v
}

fn push_packed(v: u32, dst: &mut Vec<u8>) {
    let n = (v >> 24) as usize;
    let b = v.to_be_bytes();
    dst.extend_from_slice(&b[4 - n..]);
}

/// 文字（または組）を表す検索キー。
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Key {
    One(u32),
    Two(u32, u32),
}

struct Builder {
    t: Table,
    /// 定義順の（符号, 文字）
    order: Vec<(u32, Key)>,
}

impl Builder {
    fn new(family: Family) -> Builder {
        let mut single = [INVALID; 256];
        for b in 0..0x80u32 {
            single[b as usize] = b;
        }
        Builder {
            t: Table {
                family,
                single,
                dec2: vec![0; 1 << 16],
                dec3: if family == Family::Euc {
                    vec![0; 1 << 16]
                } else {
                    Vec::new()
                },
                pairs: Vec::new(),
                enc_bmp: vec![0; 1 << 16],
                enc_astral: HashMap::new(),
                enc_pairs: HashMap::new(),
                bases: HashSet::new(),
            },
            order: Vec::new(),
        }
    }

    fn lead(&mut self, b: u8, kind: u32) {
        self.t.single[b as usize] = kind;
    }

    fn single(&mut self, b: u8, cp: u32) {
        self.t.single[b as usize] = cp;
        self.order.push((pack(&[b]), Key::One(cp)));
    }

    fn value(&mut self, key: Key) -> u32 {
        match key {
            Key::One(cp) => cp,
            Key::Two(a, b) => {
                self.t.pairs.push((a, b));
                PAIR | (self.t.pairs.len() as u32 - 1)
            }
        }
    }

    fn double(&mut self, code: u16, key: Key) {
        let v = self.value(key);
        self.t.dec2[code as usize] = v;
        self.order.push((pack(&code.to_be_bytes()), key));
    }

    fn triple(&mut self, code: u16, key: Key) {
        let v = self.value(key);
        self.t.dec3[code as usize] = v;
        let [a, b] = code.to_be_bytes();
        self.order.push((pack(&[0x8F, a, b]), key));
    }

    /// 符号表を作る。同じ文字に複数の符号がある場合は `preferred` が返す符号（なければ
    /// 最初に定義された符号）を使い、それ以外の符号には NONCANON の印を付ける。
    fn finish(mut self, preferred: &dyn Fn(Key) -> Option<u32>) -> Table {
        let mut chosen: HashMap<Key, u32> = HashMap::new();
        let defined: HashSet<(u32, Key)> = self.order.iter().copied().collect();
        for &(code, key) in &self.order {
            if chosen.contains_key(&key) {
                continue;
            }
            let pick = preferred(key)
                .filter(|p| defined.contains(&(*p, key)))
                .unwrap_or(code);
            chosen.insert(key, pick);
        }
        let order = std::mem::take(&mut self.order);
        for &(code, key) in &order {
            if chosen[&key] != code {
                self.mark_noncanonical(code);
            }
        }
        for (key, code) in chosen {
            match key {
                Key::One(cp) => self.t.set_enc(cp, code),
                Key::Two(a, b) => {
                    self.t.enc_pairs.insert((a, b), code);
                    self.t.bases.insert(a);
                }
            }
        }
        // JIS と Microsoft の対応の違いを吸収する（片方しかない場合に、もう片方も変換できるように）
        let pairs = JIS_MS.iter().map(|&(j, m, _)| (j, m)).chain(OVERLINE_YEN);
        for (jis, ms) in pairs {
            let (a, b) = (self.t.enc(jis), self.t.enc(ms));
            if a == 0 && b != 0 {
                self.t.set_enc(jis, b);
            } else if b == 0 && a != 0 {
                self.t.set_enc(ms, a);
            }
        }
        self.t
    }

    fn mark_noncanonical(&mut self, code: u32) {
        let n = code >> 24;
        let low = (code & 0xFFFF) as usize;
        match n {
            2 => self.t.dec2[low] |= NONCANON,
            3 => self.t.dec3[low] |= NONCANON,
            // 1 バイトの重複はない
            _ => {}
        }
    }
}

impl Table {
    fn enc(&self, cp: u32) -> u32 {
        if cp < 0x10000 {
            self.enc_bmp[cp as usize]
        } else {
            self.enc_astral.get(&cp).copied().unwrap_or(0)
        }
    }

    fn set_enc(&mut self, cp: u32, code: u32) {
        if cp < 0x10000 {
            self.enc_bmp[cp as usize] = code;
        } else {
            self.enc_astral.insert(cp, code);
        }
    }

    fn is_trail(&self, lead: u8, b: u8) -> bool {
        match self.family {
            Family::Sjis => matches!(b, 0x40..=0x7E | 0x80..=0xFC),
            Family::Euc => {
                let _ = lead;
                matches!(b, 0xA1..=0xFE)
            }
        }
    }

    fn emit(&self, v: u32, sink: &mut Sink<'_>) {
        if v & NONCANON != 0 {
            sink.stats.noncanonical += 1;
        }
        if v & PAIR != 0 {
            let (a, b) = self.pairs[(v & CP_MASK) as usize];
            sink.push_cp(a);
            sink.push_cp(b);
        } else {
            sink.push_cp(v & CP_MASK);
        }
    }

    /// `src` をデコードし、使ったバイト数を返す（`last` でなければ途中の文字を残す）。
    pub fn decode(&self, src: &[u8], last: bool, sink: &mut Sink<'_>) -> usize {
        let n = src.len();
        let mut i = 0;
        while i < n {
            let b = src[i];
            if b < 0x80 {
                let start = i;
                while i < n && src[i] < 0x80 {
                    i += 1;
                }
                sink.dst.extend_from_slice(&src[start..i]);
                continue;
            }
            match self.single[b as usize] {
                INVALID => {
                    sink.invalid(b);
                    i += 1;
                }
                LEAD2 => {
                    if i + 1 >= n {
                        if !last {
                            return i;
                        }
                        sink.invalid(b);
                        i += 1;
                        continue;
                    }
                    let t = src[i + 1];
                    let v = self.dec2[(b as usize) << 8 | t as usize];
                    if v != 0 {
                        self.emit(v, sink);
                        i += 2;
                    } else if self.is_trail(b, t) {
                        // 未定義の符号: 2 バイトとも保持する
                        sink.invalid(b);
                        sink.invalid(t);
                        i += 2;
                    } else {
                        // 2 バイト目になれないバイトは改めて読む
                        sink.invalid(b);
                        i += 1;
                    }
                }
                LEAD3 => {
                    if i + 2 >= n {
                        let partial_ok = src[i + 1..].iter().all(|&t| self.is_trail(b, t));
                        if !last && partial_ok {
                            return i;
                        }
                        sink.invalid(b);
                        i += 1;
                        continue;
                    }
                    let (t1, t2) = (src[i + 1], src[i + 2]);
                    let v = self.dec3[(t1 as usize) << 8 | t2 as usize];
                    if v != 0 {
                        self.emit(v, sink);
                        i += 3;
                    } else if self.is_trail(b, t1) && self.is_trail(b, t2) {
                        sink.invalid(b);
                        sink.invalid(t1);
                        sink.invalid(t2);
                        i += 3;
                    } else {
                        sink.invalid(b);
                        i += 1;
                    }
                }
                cp => {
                    sink.push_cp(cp);
                    i += 1;
                }
            }
        }
        n
    }

    /// 正しい UTF-8 の `s` をエンコードする。変換できない文字の `s` 内の範囲を報告する。
    pub fn encode(&self, s: &str, dst: &mut Vec<u8>, bad: &mut dyn FnMut(Range<usize>)) {
        dst.reserve(s.len());
        let bytes = s.as_bytes();
        let mut it = s.char_indices().peekable();
        while let Some((i, c)) = it.next() {
            let cp = c as u32;
            if cp < 0x80 {
                dst.push(bytes[i]);
                continue;
            }
            if self.bases.contains(&cp)
                && let Some(&(_, next)) = it.peek()
                && let Some(&code) = self.enc_pairs.get(&(cp, next as u32))
            {
                push_packed(code, dst);
                it.next();
                continue;
            }
            match self.enc(cp) {
                0 => bad(i..i + c.len_utf8()),
                code => push_packed(code, dst),
            }
        }
    }

    /// 入力が続く場合に次の入力まで持ち越す位置。[`Table::encode`] と同じ規則で先頭から
    /// 組を作っていき、最後の文字が組にならずに残り、次の文字と組になりうる場合はその位置。
    pub fn hold_from(&self, run: &[u8]) -> Option<usize> {
        if self.bases.is_empty() {
            return None;
        }
        let s = std::str::from_utf8(run).ok()?;
        let mut it = s.char_indices().peekable();
        let mut lone_base = None;
        while let Some((i, c)) = it.next() {
            let cp = c as u32;
            lone_base = None;
            if !self.bases.contains(&cp) {
                continue;
            }
            match it.peek() {
                Some(&(_, next)) if self.enc_pairs.contains_key(&(cp, next as u32)) => {
                    it.next();
                }
                Some(_) => {}
                None => lone_base = Some(i),
            }
        }
        lone_base
    }
}

/// `encoding_rs` で 1 文字にデコードできるバイト列ならその文字。
fn web_char(enc: &'static encoding_rs::Encoding, bytes: &[u8]) -> Option<u32> {
    let s = enc.decode_without_bom_handling_and_without_replacement(bytes)?;
    let mut cs = s.chars();
    let c = cs.next()?;
    (cs.next().is_none() && c as u32 >= 0x80).then_some(c as u32)
}

/// `encoding_rs` が選ぶ符号（WHATWG の優先順位 = Windows と同じ）。
fn web_preferred(enc: &'static encoding_rs::Encoding, key: Key) -> Option<u32> {
    let Key::One(cp) = key else { return None };
    let c = char::from_u32(cp)?;
    let mut buf = [0u8; 4];
    let (bytes, _, errors) = enc.encode(c.encode_utf8(&mut buf));
    (!errors && !bytes.is_empty()).then(|| pack(&bytes))
}

fn sjis_leads(b: &mut Builder) {
    for l in (0x81..=0x9F).chain(0xE0..=0xFC) {
        b.lead(l, LEAD2);
    }
    for (i, k) in (0xA1..=0xDFu8).enumerate() {
        b.single(k, 0xFF61 + i as u32);
    }
}

fn sjis_codes() -> impl Iterator<Item = u16> {
    (0x81..=0x9Fu16).chain(0xE0..=0xFC).flat_map(|l| {
        (0x40..=0xFCu16)
            .filter(|&t| t != 0x7F)
            .map(move |t| l << 8 | t)
    })
}

fn build_cp932() -> Table {
    let mut b = Builder::new(Family::Sjis);
    b.single(0x80, 0x80);
    sjis_leads(&mut b);
    for code in sjis_codes() {
        if let Some(cp) = web_char(encoding_rs::SHIFT_JIS, &code.to_be_bytes()) {
            b.double(code, Key::One(cp));
        }
    }
    b.finish(&|k| web_preferred(encoding_rs::SHIFT_JIS, k))
}

fn build_shift_jis() -> Table {
    let mut b = Builder::new(Family::Sjis);
    sjis_leads(&mut b);
    for code in sjis_codes() {
        // JIS X 0208 の範囲（1〜8 区、16〜84 区）だけ
        let lead = code >> 8;
        if !matches!(lead, 0x81..=0x84 | 0x88..=0x9F | 0xE0..=0xEA) {
            continue;
        }
        if let Some(cp) = web_char(encoding_rs::SHIFT_JIS, &code.to_be_bytes()) {
            let cp = JIS_MS
                .iter()
                .find(|(_, _, c)| *c == code)
                .map_or(cp, |(jis, _, _)| *jis);
            b.double(code, Key::One(cp));
        }
    }
    b.finish(&|_| None)
}

fn build_euc_jp() -> Table {
    let mut b = Builder::new(Family::Euc);
    b.lead(0x8E, LEAD2);
    b.lead(0x8F, LEAD3);
    for l in 0xA1..=0xFE {
        b.lead(l, LEAD2);
    }
    let enc = encoding_rs::EUC_JP;
    for t in 0xA1..=0xDFu8 {
        b.double(0x8E00 | t as u16, Key::One(0xFF61 + (t - 0xA1) as u32));
    }
    for l in 0xA1..=0xFEu8 {
        for t in 0xA1..=0xFEu8 {
            if let Some(cp) = web_char(enc, &[l, t]) {
                b.double(u16::from_be_bytes([l, t]), Key::One(cp));
            }
        }
    }
    for l in 0xA1..=0xFEu8 {
        for t in 0xA1..=0xFEu8 {
            if let Some(cp) = web_char(enc, &[0x8F, l, t]) {
                b.triple(u16::from_be_bytes([l, t]), Key::One(cp));
            }
        }
    }
    b.finish(&|k| web_preferred(enc, k))
}

/// Shift_JIS-2004 の符号を JIS X 0213 の（面, 区, 点）に変換する。
pub(crate) fn sjis_to_men_ku_ten(code: u16) -> (u8, u8, u8) {
    let [s1, s2] = code.to_be_bytes();
    let hi = u8::from(s2 >= 0x9F);
    let ten = if s2 >= 0x9F {
        s2 - 0x9E
    } else {
        s2 - 0x40 + 1 - u8::from(s2 >= 0x80)
    };
    let (men, ku) = match s1 {
        0x81..=0x9F => (1, (s1 - 0x81) * 2 + 1 + hi),
        0xE0..=0xEF => (1, (s1 - 0xE0) * 2 + 63 + hi),
        0xF0 => (2, if hi == 0 { 1 } else { 8 }),
        0xF1 => (2, 3 + hi),
        0xF2 => (2, if hi == 0 { 5 } else { 12 }),
        0xF3 => (2, 13 + hi),
        0xF4 => (2, if hi == 0 { 15 } else { 78 }),
        _ => (2, (s1 - 0xF5) * 2 + 79 + hi),
    };
    (men, ku, ten)
}

fn build_2004(family: Family) -> Table {
    let mut b = Builder::new(family);
    match family {
        Family::Sjis => sjis_leads(&mut b),
        Family::Euc => {
            b.lead(0x8E, LEAD2);
            b.lead(0x8F, LEAD3);
            for l in 0xA1..=0xFE {
                b.lead(l, LEAD2);
            }
            for t in 0xA1..=0xDFu8 {
                b.double(0x8E00 | t as u16, Key::One(0xFF61 + (t - 0xA1) as u32));
            }
        }
    }
    for &(code, a, c2) in JISX0213_SJIS.iter() {
        let key = if c2 == 0 {
            Key::One(a)
        } else {
            Key::Two(a, c2)
        };
        match family {
            Family::Sjis => b.double(code, key),
            Family::Euc => {
                // Shift_JIS-2004 の表は 1 バイトの 0x5C・0x7E との重複を避けて全角形にしているが、
                // EUC-JIS-2004 の表では本来の文字
                let key = match key {
                    Key::One(0xFFE3) => Key::One(0x203E),
                    Key::One(0xFFE5) => Key::One(0x00A5),
                    k => k,
                };
                let (men, ku, ten) = sjis_to_men_ku_ten(code);
                let euc = u16::from_be_bytes([ku + 0xA0, ten + 0xA0]);
                if men == 1 {
                    b.double(euc, key);
                } else {
                    b.triple(euc, key);
                }
            }
        }
    }
    b.finish(&|_| None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn men_ku_ten() {
        assert_eq!(sjis_to_men_ku_ten(0x8140), (1, 1, 1));
        assert_eq!(sjis_to_men_ku_ten(0x889F), (1, 16, 1));
        assert_eq!(sjis_to_men_ku_ten(0x9FFC), (1, 62, 94));
        assert_eq!(sjis_to_men_ku_ten(0xE040), (1, 63, 1));
        assert_eq!(sjis_to_men_ku_ten(0xEFFC), (1, 94, 94));
        assert_eq!(sjis_to_men_ku_ten(0xF040), (2, 1, 1));
        assert_eq!(sjis_to_men_ku_ten(0xF09F), (2, 8, 1));
        assert_eq!(sjis_to_men_ku_ten(0xF4FC), (2, 78, 94));
        assert_eq!(sjis_to_men_ku_ten(0xF540), (2, 79, 1));
        assert_eq!(sjis_to_men_ku_ten(0xFCFC), (2, 94, 94));
    }

    #[test]
    fn pack_roundtrip() {
        let mut v = Vec::new();
        push_packed(pack(&[0x8F, 0xA1, 0xA2]), &mut v);
        push_packed(pack(&[0x81, 0x40]), &mut v);
        push_packed(pack(&[0xA1]), &mut v);
        assert_eq!(v, [0x8F, 0xA1, 0xA2, 0x81, 0x40, 0xA1]);
    }
}
