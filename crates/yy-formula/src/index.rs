//! 大量のデータでの計算を速くする索引（15 章 7.4）。
//!
//! - [`ExactIndex`]: `XLOOKUP` の完全一致の索引（値 → 最初と最後の位置）。列が変わるまで使い回せるので、
//!   作るのは [`crate::Grid::exact_index`] を持つ側（列の版で覚えておく）。
//! - [`GroupTable`]: 同じ範囲に対する多数の `SUMIFS`・`COUNTIFS`（条件の値だけが違う集計表）を、
//!   範囲を 1 回読んで「条件の値の組 → 合計・件数」にまとめたもの。1 回の再計算の間だけ [`Cache`] に
//!   置き、同じ範囲を 2 回目に求められたときに作る。

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};
use std::ops::Range;
use std::sync::Arc;

use yy_numfmt::{DateSystem, Parsed};

use crate::parse::Area;
use crate::{Cell, Error, Grid};

/// 速いハッシュ（FxHash。索引の中だけで使う）。
#[derive(Default, Clone, Copy)]
pub(crate) struct Fx(u64);

impl Hasher for Fx {
    fn finish(&self) -> u64 {
        // 掛け算の下位ビットは入力の下位ビットでしか決まらない（整数の f64 は下位がすべて 0）ので、
        // 上位を下ろして混ぜる
        let h = self.0;
        h ^ (h >> 29) ^ (h >> 47)
    }

    fn write(&mut self, bytes: &[u8]) {
        let mut chunks = bytes.chunks_exact(8);
        for c in &mut chunks {
            self.write_u64(u64::from_le_bytes(c.try_into().expect("8 bytes")));
        }
        let rest = chunks.remainder();
        if !rest.is_empty() {
            let mut b = [0u8; 8];
            b[..rest.len()].copy_from_slice(rest);
            self.write_u64(u64::from_le_bytes(b));
        }
    }

    fn write_u64(&mut self, v: u64) {
        self.0 = (self.0.rotate_left(5) ^ v).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }

    fn write_u8(&mut self, v: u8) {
        self.write_u64(v as u64);
    }

    fn write_u32(&mut self, v: u32) {
        self.write_u64(v as u64);
    }

    fn write_usize(&mut self, v: usize) {
        self.write_u64(v as u64);
    }
}

pub(crate) type FxMap<K, V> = HashMap<K, V, BuildHasherDefault<Fx>>;

/// 値 → 番号（同じ文字列を何度も小文字にしたり数値に読んだりしないよう、元の文字列でも覚える）。
#[derive(Default)]
struct Codes {
    keys: FxMap<Key, u32>,
    raw: FxMap<Box<str>, u32>,
}

impl Codes {
    fn code(&mut self, c: Cell<'_>, to_key: impl Fn(Cell<'_>) -> Key) -> u32 {
        if let Cell::Text(s) = c
            && let Some(&k) = self.raw.get(s)
        {
            return k;
        }
        let next = self.keys.len() as u32;
        let code = *self.keys.entry(to_key(c)).or_insert(next);
        if let Cell::Text(s) = c {
            self.raw.insert(s.into(), code);
        }
        code
    }
}

/// 等しいかを比べるための値（文字列は小文字、数値は -0 を 0 に）。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    Empty,
    Num(u64),
    Text(Box<str>),
    Bool(bool),
    Err(u8),
}

impl Key {
    pub(crate) fn num(n: f64) -> Key {
        Key::Num(if n == 0.0 { 0 } else { n.to_bits() })
    }

    /// `XLOOKUP` の完全一致の値。
    pub fn exact(c: Cell<'_>) -> Key {
        match c {
            Cell::Empty => Key::Empty,
            Cell::Num(n) => Key::num(n),
            Cell::Text(s) => Key::Text(s.to_lowercase().into()),
            Cell::Bool(b) => Key::Bool(b),
            Cell::Err(e) => Key::Err(e.code()),
        }
    }

    /// `SUMIFS` の等しいの条件で比べる値（数値に読める文字列は数値、空の文字列は空）。
    pub(crate) fn criteria(c: Cell<'_>, sys: DateSystem) -> Key {
        match c {
            Cell::Text("") => Key::Empty,
            Cell::Text(s) => match yy_numfmt::parse_input(s, sys) {
                Parsed::Number(n, _) => Key::num(n),
                _ => Key::Text(s.to_lowercase().into()),
            },
            c => Key::exact(c),
        }
    }
}

/// 完全一致の索引（値 → 最初と最後の位置。位置は範囲の先頭からの数）。
#[derive(Debug, Default)]
pub struct ExactIndex {
    keys: FxMap<Key, u32>,
    /// 番号 → （最初, 最後）
    pos: Vec<(u32, u32)>,
}

impl ExactIndex {
    /// 1 列の `rows` から作る。
    pub fn build(grid: &dyn Grid, sheet: usize, col: u32, rows: Range<u64>) -> ExactIndex {
        let start = rows.start;
        let mut codes = Codes::default();
        // 伸ばしながらの作り直しを減らす
        let guess = ((rows.end - rows.start) / 4).min(1 << 22) as usize;
        codes.keys.reserve(guess);
        let mut pos: Vec<(u32, u32)> = Vec::with_capacity(guess);
        grid.scan(sheet, col, rows, &mut |r, v| {
            let i = (r - start) as u32;
            let code = codes.code(v, Key::exact) as usize;
            if code == pos.len() {
                pos.push((i, i));
            } else {
                pos[code].1 = i;
            }
        });
        ExactIndex {
            keys: codes.keys,
            pos,
        }
    }

    /// 位置（`last` なら最後の位置）。
    pub fn find(&self, key: Cell<'_>, last: bool) -> Option<usize> {
        self.keys.get(&Key::exact(key)).map(|&c| {
            let (f, l) = self.pos[c as usize];
            (if last { l } else { f }) as usize
        })
    }

    /// おおよそのメモリの大きさ（バイト）。
    pub fn heap_bytes(&self) -> usize {
        self.keys.capacity() * 40
            + self.pos.capacity() * 8
            + self
                .keys
                .keys()
                .map(|k| match k {
                    Key::Text(s) => s.len(),
                    _ => 0,
                })
                .sum::<usize>()
    }
}

/// 集計の表（条件の値の組 → 合計・件数・合計範囲のエラー）。
#[derive(Debug, Default)]
pub(crate) struct GroupTable {
    /// 列ごとの値 → 番号
    codes: Vec<FxMap<Key, u32>>,
    /// 番号の組（4 列までは 1 つの数に詰める）→ 合計・件数・エラー
    groups: FxMap<GroupId, (f64, u64, Option<Error>)>,
}

impl GroupTable {
    /// `crit` の各列（同じ行数の 1 列の範囲）と `sum`（合計範囲。`COUNTIFS` は `None`）から作る。
    pub(crate) fn build(
        grid: &dyn Grid,
        sys: DateSystem,
        crit: &[(usize, Area)],
        sum: Option<(usize, Area)>,
    ) -> GroupTable {
        let rows = crit[0].1.rows() as usize;
        let mut codes: Vec<FxMap<Key, u32>> = Vec::with_capacity(crit.len());
        let mut per_row: Vec<Vec<u32>> = Vec::with_capacity(crit.len());
        for &(sheet, a) in crit {
            let mut map = Codes::default();
            let mut col = vec![0u32; rows];
            grid.scan(sheet, a.c0, a.r0..a.r1 + 1, &mut |r, v| {
                col[(r - a.r0) as usize] = map.code(v, |c| Key::criteria(c, sys));
            });
            codes.push(map.keys);
            per_row.push(col);
        }
        let mut sums = vec![0.0f64; rows];
        let mut errs: Vec<Option<Error>> = Vec::new();
        if let Some((sheet, a)) = sum {
            errs = vec![None; rows];
            grid.scan(sheet, a.c0, a.r0..a.r1 + 1, &mut |r, v| {
                let i = (r - a.r0) as usize;
                match v {
                    Cell::Num(x) => sums[i] = x,
                    Cell::Err(e) => errs[i] = Some(e),
                    _ => {}
                }
            });
        }
        let mut groups: FxMap<GroupId, (f64, u64, Option<Error>)> = FxMap::default();
        for i in 0..rows {
            let id = group_id(per_row.iter().map(|c| c[i]));
            let e = groups.entry(id).or_insert((0.0, 0, None));
            e.0 += sums[i];
            e.1 += 1;
            if e.2.is_none()
                && let Some(x) = errs.get(i).copied().flatten()
            {
                e.2 = Some(x);
            }
        }
        GroupTable { codes, groups }
    }

    /// 条件の値の組（すべて等しい）の合計・件数。
    pub(crate) fn get(&self, keys: &[Key]) -> (f64, u64, Option<Error>) {
        let mut code = Vec::with_capacity(keys.len());
        for (m, k) in self.codes.iter().zip(keys) {
            match m.get(k) {
                Some(&c) => code.push(c),
                None => return (0.0, 0, None),
            }
        }
        self.groups
            .get(&group_id(code.into_iter()))
            .copied()
            .unwrap_or((0.0, 0, None))
    }
}

/// 条件の値の番号の組。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum GroupId {
    Packed(u128),
    Many(Vec<u32>),
}

fn group_id(codes: impl ExactSizeIterator<Item = u32>) -> GroupId {
    if codes.len() <= 4 {
        GroupId::Packed(codes.fold(0u128, |acc, c| acc << 32 | c as u128))
    } else {
        GroupId::Many(codes.collect())
    }
}

/// 範囲の組（合計範囲と条件範囲）。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct GroupKey {
    pub(crate) sum: Option<(usize, u64, u32, u64, u32)>,
    pub(crate) crit: Vec<(usize, u64, u32, u64, u32)>,
}

pub(crate) fn area_key(sheet: usize, a: &Area) -> (usize, u64, u32, u64, u32) {
    (sheet, a.r0, a.c0, a.r1, a.c1)
}

/// 求められた回数と、作った集計の表。
type GroupSlot = (u32, Option<Arc<GroupTable>>);

/// 1 回の再計算の間の覚え書き（同じ範囲の `SUMIFS` をまとめる）。
#[derive(Debug, Default)]
pub struct Cache {
    /// 範囲の組 → 求められた回数・集計の表
    groups: RefCell<HashMap<GroupKey, GroupSlot>>,
}

impl Cache {
    /// 集計の表（2 回目に求められたときに作る。1 回目は `None`）。
    pub(crate) fn group(
        &self,
        key: GroupKey,
        build: impl FnOnce() -> GroupTable,
    ) -> Option<Arc<GroupTable>> {
        let count = {
            let mut g = self.groups.borrow_mut();
            let e = g.entry(key.clone()).or_insert((0, None));
            if let Some(t) = &e.1 {
                return Some(t.clone());
            }
            e.0 += 1;
            e.0
        };
        if count < 2 {
            return None;
        }
        let t = Arc::new(build());
        self.groups
            .borrow_mut()
            .insert(key, (count, Some(t.clone())));
        Some(t)
    }
}
