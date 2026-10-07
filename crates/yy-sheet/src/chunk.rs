//! 列のチャンク（15 章 3.2）。
//!
//! 1 列の連続した行（既定で 65,536 行まで）の値。チャンクは変更しない。中身は型ごとに詰めた形で
//! ファイルに置き（[`Data::encode`]）、読むときに展開してキャッシュに置く。
//!
//! 文字列はチャンクごとの辞書（文字列の一覧）と辞書の番号で持つ。

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};

use crate::Context;
use crate::store::Region;
use crate::value::{CellError, Value};

/// チャンクの行数の上限。
pub const MAX_ROWS: usize = 65_536;

// ---- ビット列 ------------------------------------------------------------------------

/// ビット列。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Bitmap {
    words: Vec<u64>,
    len: usize,
}

impl Bitmap {
    pub fn new(len: usize, value: bool) -> Bitmap {
        let mut b = Bitmap {
            words: vec![if value { u64::MAX } else { 0 }; len.div_ceil(64)],
            len,
        };
        b.clear_tail();
        b
    }

    fn clear_tail(&mut self) {
        if self.len % 64 != 0
            && let Some(w) = self.words.last_mut()
        {
            *w &= (1u64 << (self.len % 64)) - 1;
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn get(&self, i: usize) -> bool {
        self.words[i / 64] >> (i % 64) & 1 == 1
    }

    pub fn set(&mut self, i: usize, v: bool) {
        if v {
            self.words[i / 64] |= 1 << (i % 64);
        } else {
            self.words[i / 64] &= !(1 << (i % 64));
        }
    }

    pub fn push(&mut self, v: bool) {
        if self.len % 64 == 0 {
            self.words.push(0);
        }
        self.len += 1;
        self.set(self.len - 1, v);
    }

    pub fn count_ones(&self) -> usize {
        self.words.iter().map(|w| w.count_ones() as usize).sum()
    }

    pub fn all(&self) -> bool {
        self.count_ones() == self.len
    }

    pub fn words(&self) -> &[u64] {
        &self.words
    }

    pub fn from_words(words: Vec<u64>, len: usize) -> Bitmap {
        let mut b = Bitmap { words, len };
        b.words.resize(len.div_ceil(64), 0);
        b.clear_tail();
        b
    }

    fn heap_bytes(&self) -> usize {
        self.words.len() * 8
    }
}

// ---- 辞書 ----------------------------------------------------------------------------

/// 文字列の一覧（詰めて持つ）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Dict {
    /// 文字列をつなげたもの（UTF-8 であることが型で保証される）
    bytes: String,
    /// 各文字列の終わりの位置
    ends: Vec<u32>,
}

impl Dict {
    pub fn len(&self) -> usize {
        self.ends.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ends.is_empty()
    }

    pub fn get(&self, id: u32) -> &str {
        let i = id as usize;
        let start = if i == 0 { 0 } else { self.ends[i - 1] as usize };
        let end = self.ends[i] as usize;
        &self.bytes[start..end]
    }

    pub fn push(&mut self, s: &str) -> u32 {
        self.bytes.push_str(s);
        self.ends.push(self.bytes.len() as u32);
        (self.ends.len() - 1) as u32
    }

    pub fn iter(&self) -> impl Iterator<Item = &str> {
        (0..self.len() as u32).map(|i| self.get(i))
    }

    fn heap_bytes(&self) -> usize {
        self.bytes.len() + self.ends.len() * 4
    }
}

/// 速いハッシュ（辞書づくり用。FxHash と同じ方式）。
#[derive(Default, Clone, Copy)]
pub struct FxHasher(u64);

const FX_K: u64 = 0x517c_c1b7_2722_0a95;

impl FxHasher {
    #[inline]
    fn add(&mut self, w: u64) {
        self.0 = (self.0.rotate_left(5) ^ w).wrapping_mul(FX_K);
    }
}

impl Hasher for FxHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let mut c = bytes.chunks_exact(8);
        for w in &mut c {
            self.add(u64::from_le_bytes(w.try_into().unwrap()));
        }
        let r = c.remainder();
        if !r.is_empty() {
            let mut b = [0u8; 8];
            b[..r.len()].copy_from_slice(r);
            self.add(u64::from_le_bytes(b));
        }
    }
    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.add(i as u64);
    }
    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.add(i as u64);
    }
    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }
}

/// 速いハッシュの表。
pub type FxMap<K, V> = HashMap<K, V, BuildHasherDefault<FxHasher>>;

fn fx_str(s: &str) -> u64 {
    let mut h = FxHasher::default();
    h.write(s.as_bytes());
    h.finish()
}

/// ハッシュの上位のビットで場所を決める（FxHash の下位のビットは偏るため）。
fn slot_of(h: u64, cap: usize) -> usize {
    (h >> (64 - cap.trailing_zeros())) as usize
}

/// 辞書の索引（文字列から番号。開番地法で、文字列そのものは辞書にだけ持つ）。
#[derive(Debug, Default)]
struct DictIndex {
    /// 番号 + 1（0 は空き）
    slots: Vec<u32>,
}

impl DictIndex {
    fn find_or_insert(&mut self, dict: &mut Dict, s: &str) -> u32 {
        if (dict.len() + 1) * 2 > self.slots.len() {
            self.grow(dict);
        }
        let mask = self.slots.len() - 1;
        let mut i = slot_of(fx_str(s), self.slots.len());
        loop {
            let v = self.slots[i];
            if v == 0 {
                let id = dict.push(s);
                self.slots[i] = id + 1;
                return id;
            }
            if dict.get(v - 1) == s {
                return v - 1;
            }
            i = (i + 1) & mask;
        }
    }

    fn grow(&mut self, dict: &Dict) {
        let cap = (self.slots.len() * 2).max(1024);
        let mut slots = vec![0u32; cap];
        let mask = cap - 1;
        for id in 0..dict.len() as u32 {
            let mut i = slot_of(fx_str(dict.get(id)), cap);
            while slots[i] != 0 {
                i = (i + 1) & mask;
            }
            slots[i] = id + 1;
        }
        self.slots = slots;
    }
}

// ---- 値の参照 ------------------------------------------------------------------------

/// セルの値の参照（文字列をコピーしない）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CellRef<'a> {
    Empty,
    Number(f64),
    Text(&'a str),
    Bool(bool),
    Error(CellError),
}

impl CellRef<'_> {
    pub fn to_value(self) -> Value {
        match self {
            CellRef::Empty => Value::Empty,
            CellRef::Number(n) => Value::Number(n),
            CellRef::Text(s) => Value::text(s),
            CellRef::Bool(b) => Value::Bool(b),
            CellRef::Error(e) => Value::Error(e),
        }
    }

    pub fn of(v: &Value) -> CellRef<'_> {
        match v {
            Value::Empty => CellRef::Empty,
            Value::Number(n) => CellRef::Number(*n),
            Value::Text(s) => CellRef::Text(s),
            Value::Bool(b) => CellRef::Bool(*b),
            Value::Error(e) => CellRef::Error(*e),
        }
    }
}

// ---- 中身 ----------------------------------------------------------------------------

/// チャンクの中身（展開した形）。`present` が `None` なら全行に値がある（`false` の行は空）。
#[derive(Clone, Debug, PartialEq)]
pub enum Data {
    Empty(usize),
    Number {
        vals: Vec<f64>,
        present: Option<Bitmap>,
    },
    Text {
        dict: Dict,
        ids: Vec<u32>,
        present: Option<Bitmap>,
    },
    Bool {
        bits: Bitmap,
        present: Option<Bitmap>,
    },
    Mixed(Vec<Value>),
}

impl Data {
    pub fn len(&self) -> usize {
        match self {
            Data::Empty(n) => *n,
            Data::Number { vals, .. } => vals.len(),
            Data::Text { ids, .. } => ids.len(),
            Data::Bool { bits, .. } => bits.len(),
            Data::Mixed(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn present(&self) -> Option<&Bitmap> {
        match self {
            Data::Number { present, .. }
            | Data::Text { present, .. }
            | Data::Bool { present, .. } => present.as_ref(),
            _ => None,
        }
    }

    pub fn get(&self, i: usize) -> CellRef<'_> {
        if let Some(p) = self.present()
            && !p.get(i)
        {
            return CellRef::Empty;
        }
        match self {
            Data::Empty(_) => CellRef::Empty,
            Data::Number { vals, .. } => CellRef::Number(vals[i]),
            Data::Text { dict, ids, .. } => CellRef::Text(dict.get(ids[i])),
            Data::Bool { bits, .. } => CellRef::Bool(bits.get(i)),
            Data::Mixed(v) => CellRef::of(&v[i]),
        }
    }

    pub fn heap_bytes(&self) -> usize {
        let p = self.present().map_or(0, Bitmap::heap_bytes);
        p + match self {
            Data::Empty(_) => 0,
            Data::Number { vals, .. } => vals.len() * 8,
            Data::Text { dict, ids, .. } => dict.heap_bytes() + ids.len() * 4,
            Data::Bool { bits, .. } => bits.heap_bytes(),
            Data::Mixed(v) => {
                v.len() * 32
                    + v.iter()
                        .map(|x| match x {
                            Value::Text(s) => s.len(),
                            _ => 0,
                        })
                        .sum::<usize>()
            }
        }
    }

    /// 値の並びから、いちばん詰められる形を選んで作る。
    pub fn from_values<'a>(values: impl IntoIterator<Item = CellRef<'a>>) -> Data {
        let mut b = Builder::default();
        for v in values {
            b.push(v);
        }
        b.finish()
    }

    /// 統計。
    pub fn stats(&self) -> Stats {
        let mut s = Stats::default();
        for i in 0..self.len() {
            s.add(self.get(i));
        }
        s
    }
}

// ---- 作り方 --------------------------------------------------------------------------

/// 値を 1 つずつ足してチャンクの中身を作る。型が揃っていれば詰めた形、揃わなければ混在。
#[derive(Debug, Default)]
pub struct Builder {
    kind: Kind,
    len: usize,
    present: Bitmap,
    any_empty: bool,
    nums: Vec<f64>,
    dict: Dict,
    dict_index: DictIndex,
    ids: Vec<u32>,
    bools: Bitmap,
    mixed: Vec<Value>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Kind {
    #[default]
    Empty,
    Number,
    Text,
    Bool,
    Mixed,
}

impl Builder {
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn push(&mut self, v: CellRef<'_>) {
        let want = match v {
            CellRef::Empty => None,
            CellRef::Number(_) => Some(Kind::Number),
            CellRef::Text(_) => Some(Kind::Text),
            CellRef::Bool(_) => Some(Kind::Bool),
            CellRef::Error(_) => Some(Kind::Mixed),
        };
        if let Some(k) = want
            && self.kind != k
            && self.kind != Kind::Mixed
        {
            if self.kind == Kind::Empty {
                self.kind = k;
                // それまでの空の行を埋める
                for _ in 0..self.len {
                    self.push_typed_empty();
                }
            } else {
                self.switch_to_mixed();
            }
        }
        if self.kind == Kind::Mixed {
            self.mixed.push(v.to_value());
            self.len += 1;
            return;
        }
        self.present.push(want.is_some());
        if want.is_none() {
            self.any_empty = true;
        }
        match (self.kind, v) {
            (Kind::Empty, _) => {}
            (Kind::Number, CellRef::Number(n)) => self.nums.push(n),
            (Kind::Text, CellRef::Text(s)) => {
                let id = self.dict_index.find_or_insert(&mut self.dict, s);
                self.ids.push(id);
            }
            (Kind::Bool, CellRef::Bool(b)) => self.bools.push(b),
            _ => self.push_typed_empty(),
        }
        self.len += 1;
    }

    fn push_typed_empty(&mut self) {
        match self.kind {
            Kind::Number => self.nums.push(0.0),
            Kind::Text => self.ids.push(0),
            Kind::Bool => self.bools.push(false),
            _ => {}
        }
    }

    fn switch_to_mixed(&mut self) {
        let data = std::mem::take(self).finish();
        let len = data.len();
        let mut m = Vec::with_capacity(len + 1);
        for i in 0..len {
            m.push(data.get(i).to_value());
        }
        self.kind = Kind::Mixed;
        self.len = len;
        self.mixed = m;
    }

    pub fn finish(self) -> Data {
        let present = self.any_empty.then_some(self.present);
        match self.kind {
            Kind::Empty => Data::Empty(self.len),
            Kind::Number => Data::Number {
                vals: self.nums,
                present,
            },
            Kind::Text => Data::Text {
                dict: self.dict,
                ids: self.ids,
                present,
            },
            Kind::Bool => Data::Bool {
                bits: self.bools,
                present,
            },
            Kind::Mixed => Data::Mixed(self.mixed),
        }
    }
}

// ---- 統計 ----------------------------------------------------------------------------

/// チャンクの統計（絞り込み・集計で読み飛ばすのに使う）。
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stats {
    /// 空でないセルの数
    pub nonempty: u32,
    /// 数値の最小・最大（数値がなければ `+∞`・`-∞`）
    pub min: f64,
    pub max: f64,
    /// 含む型（1: 数値、2: 文字列、4: 真偽値、8: エラー）
    pub kinds: u8,
}

impl Default for Stats {
    fn default() -> Self {
        Stats {
            nonempty: 0,
            min: f64::INFINITY,
            max: f64::NEG_INFINITY,
            kinds: 0,
        }
    }
}

impl Stats {
    pub const NUMBER: u8 = 1;
    pub const TEXT: u8 = 2;
    pub const BOOL: u8 = 4;
    pub const ERROR: u8 = 8;

    pub fn add(&mut self, v: CellRef<'_>) {
        match v {
            CellRef::Empty => return,
            CellRef::Number(n) => {
                self.kinds |= Stats::NUMBER;
                self.min = self.min.min(n);
                self.max = self.max.max(n);
            }
            CellRef::Text(_) => self.kinds |= Stats::TEXT,
            CellRef::Bool(_) => self.kinds |= Stats::BOOL,
            CellRef::Error(_) => self.kinds |= Stats::ERROR,
        }
        self.nonempty += 1;
    }
}

// ---- 書き方・読み方 ------------------------------------------------------------------

const K_EMPTY: u8 = 0;
const K_NUMBER: u8 = 1;
const K_TEXT: u8 = 2;
const K_BOOL: u8 = 3;
const K_MIXED: u8 = 4;

const F_PRESENT: u8 = 1;
/// 数値を i32 で持つ
const F_I32: u8 = 2;
/// 辞書の番号の幅（ビット 2〜3: 0 = u8、1 = u16、2 = u32）
const F_ID_SHIFT: u8 = 2;

struct W(Vec<u8>);

impl W {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn bytes(&mut self, b: &[u8]) {
        self.0.extend_from_slice(b);
    }
    fn bitmap(&mut self, b: &Bitmap) {
        for &w in b.words() {
            self.u64(w);
        }
    }
}

struct R<'a> {
    b: &'a [u8],
    i: usize,
}

fn bad() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "チャンクの形式が正しくありません",
    )
}

impl R<'_> {
    fn take(&mut self, n: usize) -> io::Result<&[u8]> {
        let s = self.b.get(self.i..self.i + n).ok_or_else(bad)?;
        self.i += n;
        Ok(s)
    }
    fn u8(&mut self) -> io::Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> io::Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn bitmap(&mut self, len: usize) -> io::Result<Bitmap> {
        let n = len.div_ceil(64);
        let mut w = Vec::with_capacity(n);
        for _ in 0..n {
            w.push(self.u64()?);
        }
        Ok(Bitmap::from_words(w, len))
    }
}

impl Data {
    /// ファイルに置く形にする。
    pub fn encode(&self) -> Vec<u8> {
        let mut w = W(Vec::with_capacity(self.heap_bytes() + 16));
        let (kind, mut flags) = match self {
            Data::Empty(_) => (K_EMPTY, 0),
            Data::Number { .. } => (K_NUMBER, 0),
            Data::Text { .. } => (K_TEXT, 0),
            Data::Bool { .. } => (K_BOOL, 0),
            Data::Mixed(_) => (K_MIXED, 0),
        };
        if self.present().is_some() {
            flags |= F_PRESENT;
        }
        let ints = match self {
            Data::Number { vals, .. } => vals.iter().all(|&v| {
                v.fract() == 0.0
                    && (i32::MIN as f64..=i32::MAX as f64).contains(&v)
                    && !(v == 0.0 && v.is_sign_negative())
            }),
            _ => false,
        };
        if ints {
            flags |= F_I32;
        }
        let id_width = match self {
            Data::Text { dict, .. } if dict.len() <= 256 => 0,
            Data::Text { dict, .. } if dict.len() <= 65_536 => 1,
            _ => 2,
        };
        flags |= id_width << F_ID_SHIFT;
        w.u8(kind);
        w.u8(flags);
        w.u8(0);
        w.u8(0);
        w.u32(self.len() as u32);
        if let Some(p) = self.present() {
            w.bitmap(p);
        }
        match self {
            Data::Empty(_) => {}
            Data::Number { vals, .. } => {
                if ints {
                    for &v in vals {
                        w.bytes(&(v as i32).to_le_bytes());
                    }
                } else {
                    for &v in vals {
                        w.bytes(&v.to_le_bytes());
                    }
                }
            }
            Data::Text { dict, ids, .. } => {
                w.u32(dict.len() as u32);
                w.u32(dict.bytes.len() as u32);
                for &e in &dict.ends {
                    w.u32(e);
                }
                w.bytes(dict.bytes.as_bytes());
                for &id in ids {
                    match id_width {
                        0 => w.u8(id as u8),
                        1 => w.bytes(&(id as u16).to_le_bytes()),
                        _ => w.u32(id),
                    }
                }
            }
            Data::Bool { bits, .. } => w.bitmap(bits),
            Data::Mixed(vals) => {
                for v in vals {
                    match v {
                        Value::Empty => w.u8(0),
                        Value::Number(n) => {
                            w.u8(1);
                            w.bytes(&n.to_le_bytes());
                        }
                        Value::Text(s) => {
                            w.u8(2);
                            w.u32(s.len() as u32);
                            w.bytes(s.as_bytes());
                        }
                        Value::Bool(b) => {
                            w.u8(3);
                            w.u8(*b as u8);
                        }
                        Value::Error(e) => {
                            w.u8(4);
                            w.u8(e.code());
                        }
                    }
                }
            }
        }
        w.0
    }

    /// ファイルに置いた形から読む。
    pub fn decode(b: &[u8]) -> io::Result<Data> {
        let mut r = R { b, i: 0 };
        let kind = r.u8()?;
        let flags = r.u8()?;
        r.take(2)?;
        let len = r.u32()? as usize;
        if len > MAX_ROWS * 64 {
            return Err(bad());
        }
        let present = if flags & F_PRESENT != 0 {
            Some(r.bitmap(len)?)
        } else {
            None
        };
        Ok(match kind {
            K_EMPTY => Data::Empty(len),
            K_NUMBER => {
                let vals = if flags & F_I32 != 0 {
                    r.take(len * 4)?
                        .chunks_exact(4)
                        .map(|c| i32::from_le_bytes(c.try_into().unwrap()) as f64)
                        .collect()
                } else {
                    r.take(len * 8)?
                        .chunks_exact(8)
                        .map(|c| f64::from_le_bytes(c.try_into().unwrap()))
                        .collect()
                };
                Data::Number { vals, present }
            }
            K_TEXT => {
                let n = r.u32()? as usize;
                let nbytes = r.u32()? as usize;
                let mut ends = Vec::with_capacity(n);
                for _ in 0..n {
                    ends.push(r.u32()?);
                }
                let bytes = String::from_utf8(r.take(nbytes)?.to_vec()).map_err(|_| bad())?;
                if ends.windows(2).any(|w| w[0] > w[1])
                    || ends.iter().any(|&e| !bytes.is_char_boundary(e as usize))
                    || ends.last().is_some_and(|&e| e as usize != nbytes)
                {
                    return Err(bad());
                }
                let width = (flags >> F_ID_SHIFT) & 3;
                let raw = r.take(len * [1, 2, 4][width.min(2) as usize])?;
                let ids: Vec<u32> = match width {
                    0 => raw.iter().map(|&x| x as u32).collect(),
                    1 => raw
                        .chunks_exact(2)
                        .map(|c| u16::from_le_bytes([c[0], c[1]]) as u32)
                        .collect(),
                    _ => raw
                        .chunks_exact(4)
                        .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
                        .collect(),
                };
                if ids.iter().any(|&id| id as usize >= n.max(1)) && n > 0 {
                    return Err(bad());
                }
                let dict = Dict { bytes, ends };
                if n == 0 && present.as_ref().is_none_or(|p| p.count_ones() > 0) && len > 0 {
                    return Err(bad());
                }
                Data::Text { dict, ids, present }
            }
            K_BOOL => Data::Bool {
                bits: r.bitmap(len)?,
                present,
            },
            K_MIXED => {
                let mut v = Vec::with_capacity(len);
                for _ in 0..len {
                    v.push(match r.u8()? {
                        0 => Value::Empty,
                        1 => Value::Number(f64::from_le_bytes(r.take(8)?.try_into().unwrap())),
                        2 => {
                            let n = r.u32()? as usize;
                            let s = std::str::from_utf8(r.take(n)?).map_err(|_| bad())?;
                            Value::text(s)
                        }
                        3 => Value::Bool(r.u8()? != 0),
                        4 => Value::Error(CellError::from_code(r.u8()?).ok_or_else(bad)?),
                        _ => return Err(bad()),
                    });
                }
                Data::Mixed(v)
            }
            _ => return Err(bad()),
        })
    }
}

// ---- チャンク ------------------------------------------------------------------------

static NEXT_CHUNK: AtomicU64 = AtomicU64::new(1);

/// チャンク（置き場所と統計。中身は読むときに展開する）。
#[derive(Debug)]
pub struct Chunk {
    pub id: u64,
    pub rows: u32,
    pub stats: Stats,
    loc: RwLock<Region>,
}

impl Chunk {
    /// 置き場所から（ファイルを開いたとき）。
    pub fn stored(rows: u32, stats: Stats, loc: Region) -> Arc<Chunk> {
        Arc::new(Chunk {
            id: NEXT_CHUNK.fetch_add(1, Ordering::Relaxed),
            rows,
            stats,
            loc: RwLock::new(loc),
        })
    }

    /// 中身から新しいチャンクを作り、作業ファイルに書く（キャッシュにも入れる）。
    pub fn create(ctx: &Context, data: Data) -> io::Result<Arc<Chunk>> {
        let bytes = data.encode();
        let store = ctx.work()?;
        let offset = store.append(&bytes)?;
        let c = Chunk::stored(
            data.len() as u32,
            data.stats(),
            Region {
                store,
                offset,
                len: bytes.len() as u64,
            },
        );
        ctx.cache.put(c.id, Arc::new(data), &ctx.budget);
        Ok(c)
    }

    pub fn loc(&self) -> Region {
        self.loc.read().unwrap().clone()
    }

    /// 置き場所を変える（保存で新しいファイルに写したとき）。
    pub fn relocate(&self, loc: Region) {
        *self.loc.write().unwrap() = loc;
    }

    /// 中身（キャッシュになければ読んで展開する）。
    pub fn data(&self, ctx: &Context) -> io::Result<Arc<Data>> {
        if let Some(d) = ctx.cache.get(self.id) {
            return Ok(d);
        }
        let bytes = self.loc().read()?;
        let d = Arc::new(Data::decode(&bytes)?);
        if d.len() != self.rows as usize {
            return Err(bad());
        }
        ctx.cache.put(self.id, d.clone(), &ctx.budget);
        Ok(d)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(values: &[Value]) -> Data {
        let d = Data::from_values(values.iter().map(CellRef::of));
        let back = Data::decode(&d.encode()).unwrap();
        assert_eq!(back, d);
        for (i, v) in values.iter().enumerate() {
            assert_eq!(back.get(i).to_value(), *v, "row {i}");
        }
        d
    }

    #[test]
    fn encodes_each_kind() {
        let d = round_trip(&[1.0.into(), 2.5.into(), Value::Empty, (-3.0).into()]);
        assert!(matches!(
            d,
            Data::Number {
                present: Some(_),
                ..
            }
        ));
        let d = round_trip(&[1.0.into(), 2.0.into(), 1e10.into()]);
        assert!(matches!(d, Data::Number { present: None, .. }));
        let d = round_trip(&["東京".into(), "大阪".into(), "東京".into(), Value::Empty]);
        match &d {
            Data::Text { dict, .. } => assert_eq!(dict.len(), 2),
            _ => panic!(),
        }
        round_trip(&[true.into(), false.into(), Value::Empty]);
        let d = round_trip(&[
            1.0.into(),
            "a".into(),
            Value::Error(CellError::NA),
            Value::Empty,
            true.into(),
        ]);
        assert!(matches!(d, Data::Mixed(_)));
        round_trip(&[Value::Empty, Value::Empty]);
        let d = round_trip(&[Value::Empty, 5.0.into()]);
        assert_eq!(d.stats().nonempty, 1);
        assert_eq!((d.stats().min, d.stats().max), (5.0, 5.0));
        // 辞書が大きいとき
        let many: Vec<Value> = (0..70_000).map(|i| Value::text(&format!("s{i}"))).collect();
        round_trip(&many);
        round_trip(&[(-0.0).into(), 0.5.into()]);
    }

    #[test]
    fn rejects_corrupt_data() {
        let d = Data::from_values(["a", "b"].iter().map(|s| CellRef::Text(s)));
        let mut b = d.encode();
        b.truncate(b.len() - 1);
        assert!(Data::decode(&b).is_err());
        assert!(Data::decode(&[9, 0, 0, 0, 1, 0, 0, 0]).is_err());
    }
}
