//! 中身の索引（全文検索。18 章 8.3 の将来の拡張）。
//!
//! 日本語は語の区切りがないので、隣り合う 2 文字（バイグラム）を 65,536 個の区分けに丸めて、区分けごとに
//! それを含むファイルの一覧（転置索引）を持つ。探すときは、探す文字列のバイグラムをすべて含むファイルだけを
//! 候補にし、候補は実際に読んで確かめる（[`crate::search::search_content_indexed`]）。区分けに丸めるので
//! 余計な候補は出るが、索引が新しければ見落としはない。
//!
//! * 大文字・小文字はそろえて数える（大文字・小文字を区別する検索でも、候補が増えるだけ）。
//! * 索引にないファイル・変わったファイルは、検索のついでに読んで足す（初回は今までと同じ、2 回目から速い）。
//! * 消えたファイル・変わる前の記録は「消した」にしておき、多くなったら詰め直す。

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::scan::{Catalog, FileEntry};

/// 区分けの数。
pub const BUCKETS: usize = 1 << 16;

const MAGIC: &[u8; 8] = b"YYFMFTI1";

/// 1 ファイルの記録の状態。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FtState {
    /// 文字列を索引にした
    Text,
    /// バイナリ・読めない（中身は探さない）
    Binary,
    /// 大きすぎて読まなかった（そのときの上限）
    TooBig(u64),
    /// 消えた・変わる前の記録
    Dead,
}

/// 1 ファイルの記録。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FtFile {
    pub rel: String,
    pub size: u64,
    pub mtime: i64,
    pub state: FtState,
}

/// 1 つのルートの索引。
#[derive(Clone, Debug, Default)]
pub struct FtIndex {
    pub root: PathBuf,
    files: Vec<FtFile>,
    /// 区分け → ファイルの番号（小さい順）
    post: Vec<Vec<u32>>,
    by_rel: HashMap<String, u32>,
}

/// 保存する形（番号は差分にして小さくする）。
#[derive(Serialize, Deserialize)]
struct Stored {
    root: PathBuf,
    files: Vec<FtFile>,
    post: Vec<Vec<u32>>,
}

/// 大文字・小文字をそろえる（大文字にしてから小文字に。`ſ`・ケルビン記号なども普通の文字にそろう）。
fn fold(c: char) -> char {
    let u = c.to_uppercase().next().unwrap_or(c);
    u.to_lowercase().next().unwrap_or(u)
}

fn bucket(a: char, b: char) -> u16 {
    let x = (fold(a) as u32).wrapping_mul(0x9E37_79B1) ^ (fold(b) as u32).wrapping_mul(0x85EB_CA77);
    let x = x ^ (x >> 15);
    (x.wrapping_mul(0xC2B2_AE3D) >> 16) as u16
}

/// 文字列のバイグラムの区分けを集める（流し込みながら。UTF-8 の文字が区切りをまたいでもよい）。
pub struct GramSet {
    bits: Vec<u64>,
    prev: Option<char>,
    tail: Vec<u8>,
}

impl Default for GramSet {
    fn default() -> Self {
        GramSet {
            bits: vec![0; BUCKETS / 64],
            prev: None,
            tail: Vec::new(),
        }
    }
}

impl GramSet {
    fn push_char(&mut self, c: char) {
        if let Some(p) = self.prev {
            let b = bucket(p, c) as usize;
            self.bits[b / 64] |= 1 << (b % 64);
        }
        self.prev = Some(c);
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        let mut buf;
        let data: &[u8] = if self.tail.is_empty() {
            bytes
        } else {
            buf = std::mem::take(&mut self.tail);
            buf.extend_from_slice(bytes);
            &buf
        };
        let mut rest = data;
        loop {
            match std::str::from_utf8(rest) {
                Ok(s) => {
                    s.chars().for_each(|c| self.push_char(c));
                    break;
                }
                Err(e) => {
                    let (ok, after) = rest.split_at(e.valid_up_to());
                    // valid_up_to までは正しい UTF-8
                    std::str::from_utf8(ok)
                        .unwrap_or_default()
                        .chars()
                        .for_each(|c| self.push_char(c));
                    match e.error_len() {
                        // 途中で切れた文字は次へ持ち越す
                        None => {
                            self.tail = after.to_vec();
                            break;
                        }
                        Some(n) => {
                            self.push_char('\u{FFFD}');
                            rest = &after[n..];
                        }
                    }
                }
            }
        }
    }

    /// 区分け（小さい順）。
    pub fn finish(self) -> Vec<u16> {
        let mut out = Vec::new();
        for (w, &v) in self.bits.iter().enumerate() {
            let mut v = v;
            while v != 0 {
                let t = v.trailing_zeros() as usize;
                out.push((w * 64 + t) as u16);
                v &= v - 1;
            }
        }
        out
    }
}

/// 探す文字列のバイグラムの区分け（2 文字未満なら `None`。索引を使えない）。
pub fn query_grams(pattern: &str) -> Option<Vec<u16>> {
    let cs: Vec<char> = pattern.chars().collect();
    if cs.len() < 2 {
        return None;
    }
    let mut v: Vec<u16> = cs.windows(2).map(|w| bucket(w[0], w[1])).collect();
    v.sort_unstable();
    v.dedup();
    Some(v)
}

/// 索引のファイルの置き場所。
pub fn path_for(dir: &Path, root: &Path) -> PathBuf {
    crate::index::path_for(dir, root).with_extension("fti")
}

impl FtIndex {
    pub fn new(root: &Path) -> FtIndex {
        FtIndex {
            root: root.to_path_buf(),
            files: Vec::new(),
            post: vec![Vec::new(); BUCKETS],
            by_rel: HashMap::new(),
        }
    }

    pub fn load(path: &Path) -> io::Result<FtIndex> {
        let raw = std::fs::read(path)?;
        let bad = |m: String| io::Error::new(io::ErrorKind::InvalidData, m);
        let body = raw
            .strip_prefix(MAGIC.as_slice())
            .ok_or_else(|| bad("索引の形式が違います".into()))?;
        let plain =
            miniz_oxide::inflate::decompress_to_vec(body).map_err(|e| bad(format!("{e:?}")))?;
        let st: Stored = postcard::from_bytes(&plain).map_err(|e| bad(e.to_string()))?;
        if st.post.len() != BUCKETS {
            return Err(bad("索引の区分けの数が違います".into()));
        }
        let post = st
            .post
            .into_iter()
            .map(|d| {
                let mut acc = 0u32;
                d.into_iter()
                    .map(|x| {
                        acc = acc.wrapping_add(x);
                        acc
                    })
                    .collect()
            })
            .collect();
        let mut ix = FtIndex {
            root: st.root,
            files: st.files,
            post,
            by_rel: HashMap::new(),
        };
        ix.rebuild_map();
        Ok(ix)
    }

    /// 保存する（消した記録が多ければ詰め直してから）。
    pub fn save(&mut self, path: &Path) -> io::Result<()> {
        let dead = self
            .files
            .iter()
            .filter(|f| f.state == FtState::Dead)
            .count();
        if dead > 0 && dead * 3 >= self.files.len() {
            self.compact();
        }
        let st = Stored {
            root: self.root.clone(),
            files: self.files.clone(),
            post: self
                .post
                .iter()
                .map(|ids| {
                    let mut prev = 0u32;
                    ids.iter()
                        .map(|&x| {
                            let d = x.wrapping_sub(prev);
                            prev = x;
                            d
                        })
                        .collect()
                })
                .collect(),
        };
        let plain = postcard::to_allocvec(&st).map_err(io::Error::other)?;
        let packed = miniz_oxide::deflate::compress_to_vec(&plain, 3);
        crate::index::write_atomic(path, &[MAGIC.as_slice(), &packed].concat())
    }

    fn rebuild_map(&mut self) {
        self.by_rel = self
            .files
            .iter()
            .enumerate()
            .filter(|(_, f)| f.state != FtState::Dead)
            .map(|(i, f)| (f.rel.clone(), i as u32))
            .collect();
    }

    /// 消した記録を除いて番号を詰める（中身を読み直さない）。
    fn compact(&mut self) {
        let mut map = vec![u32::MAX; self.files.len()];
        let mut files = Vec::new();
        for (i, f) in self.files.iter().enumerate() {
            if f.state != FtState::Dead {
                map[i] = files.len() as u32;
                files.push(f.clone());
            }
        }
        for ids in &mut self.post {
            *ids = ids
                .iter()
                .filter_map(|&i| Some(map[i as usize]).filter(|&m| m != u32::MAX))
                .collect();
        }
        self.files = files;
        self.rebuild_map();
    }

    /// 記録の数（消したものを除く）。
    pub fn len(&self) -> usize {
        self.by_rel.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_rel.is_empty()
    }

    /// 今のファイル（大きさ・日時が同じ）の記録。
    pub fn lookup(&self, e: &FileEntry) -> Option<(u32, FtState)> {
        let &id = self.by_rel.get(&e.rel)?;
        let f = &self.files[id as usize];
        (f.size == e.meta.size && f.mtime == e.meta.mtime).then_some((id, f.state))
    }

    /// 記録 `id` が区分けをすべて含むか。
    pub fn has_all(&self, id: u32, grams: &[u16]) -> bool {
        grams
            .iter()
            .all(|&g| self.post[g as usize].binary_search(&id).is_ok())
    }

    /// ファイルの記録を足す（前の記録は消したにする）。
    pub fn add(&mut self, e: &FileEntry, state: FtState, grams: &[u16]) {
        if let Some(&old) = self.by_rel.get(&e.rel) {
            self.files[old as usize].state = FtState::Dead;
        }
        let id = self.files.len() as u32;
        self.files.push(FtFile {
            rel: e.rel.clone(),
            size: e.meta.size,
            mtime: e.meta.mtime,
            state,
        });
        for &g in grams {
            self.post[g as usize].push(id);
        }
        self.by_rel.insert(e.rel.clone(), id);
    }

    /// 目録にないファイルの記録を消したにする。
    pub fn retain_catalog(&mut self, cat: &Catalog) {
        let keep: std::collections::HashSet<&str> =
            cat.files.iter().map(|f| f.rel.as_str()).collect();
        let gone: Vec<String> = self
            .by_rel
            .keys()
            .filter(|r| !keep.contains(r.as_str()))
            .cloned()
            .collect();
        for r in gone {
            if let Some(id) = self.by_rel.remove(&r) {
                self.files[id as usize].state = FtState::Dead;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::Meta;

    fn entry(rel: &str, size: u64, mtime: i64) -> FileEntry {
        FileEntry {
            rel: rel.into(),
            meta: Meta {
                size,
                mtime,
                ..Meta::default()
            },
        }
    }

    fn grams(s: &str) -> Vec<u16> {
        let mut g = GramSet::default();
        g.feed(s.as_bytes());
        g.finish()
    }

    #[test]
    fn grams_cross_chunks_and_fold_case() {
        let text = "見積書の税込金額 Total";
        let whole = grams(text);
        // 1 バイトずつ流し込んでも同じ（UTF-8 の文字が区切りをまたぐ）
        let mut g = GramSet::default();
        for b in text.as_bytes() {
            g.feed(std::slice::from_ref(b));
        }
        assert_eq!(g.finish(), whole);
        for q in ["税込", "見積書", "total", "TOTAL", "込金額"] {
            let qg = query_grams(q).unwrap();
            assert!(qg.iter().all(|x| whole.binary_search(x).is_ok()), "{q}");
        }
        assert!(query_grams("税").is_none());
        // 壊れた UTF-8 でも落ちない
        let mut g = GramSet::default();
        g.feed(&[0xE3, 0x81, 0xFF, b'a', b'b']);
        assert!(!g.finish().is_empty());
    }

    #[test]
    fn adds_finds_compacts_and_saves() {
        let d = tempfile::tempdir().unwrap();
        let mut ix = FtIndex::new(Path::new("/r"));
        let a = entry("a.txt", 10, 1);
        let b = entry("b.txt", 20, 1);
        ix.add(&a, FtState::Text, &grams("税込の金額"));
        ix.add(&b, FtState::Text, &grams("税抜の金額"));
        let q = query_grams("税込").unwrap();
        let (ia, _) = ix.lookup(&a).unwrap();
        let (ib, _) = ix.lookup(&b).unwrap();
        assert!(ix.has_all(ia, &q));
        assert!(!ix.has_all(ib, &q));
        // 変わったファイルは記録を使わない
        assert!(ix.lookup(&entry("a.txt", 11, 1)).is_none());
        // 書き換えて足し直す・消えたものを消す
        let a2 = entry("a.txt", 12, 2);
        ix.add(&a2, FtState::Text, &grams("税抜"));
        let cat = Catalog {
            root: "/r".into(),
            files: vec![a2.clone()],
            ..Catalog::default()
        };
        ix.retain_catalog(&cat);
        assert_eq!(ix.len(), 1);
        let p = d.path().join("x.fti");
        ix.save(&p).unwrap(); // 消した記録が多いので詰め直す
        assert_eq!(ix.files.len(), 1);
        let back = FtIndex::load(&p).unwrap();
        let (id, st) = back.lookup(&a2).unwrap();
        assert_eq!(st, FtState::Text);
        assert!(back.has_all(id, &query_grams("税抜").unwrap()));
        assert!(!back.has_all(id, &query_grams("税込").unwrap()));
        std::fs::write(&p, b"broken").unwrap();
        assert!(FtIndex::load(&p).is_err());
    }
}
