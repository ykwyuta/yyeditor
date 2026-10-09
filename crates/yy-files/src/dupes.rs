//! 完全に同じファイル（18 章 6）。
//!
//! 3 段で絞る: (1) 大きさが同じ → (2) 大きさと先頭・末尾 64 KiB のハッシュが同じ → (3) 全体のハッシュが
//! 同じ。ハッシュは索引（[`crate::index`]）に残し、変わっていないファイルは読み直さない。同じファイル ID
//! （ハードリンク）は重複に数えない。

use std::collections::HashMap;
use std::io::{self, Read};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use rayon::prelude::*;

use crate::fs::Fs;
use crate::hash::{self, Hash};
use crate::index::Index;
use crate::scan::Catalog;

/// 目録の中の 1 ファイル（何番目の目録の、何番目のファイルか）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FileRef {
    pub root: usize,
    pub index: usize,
}

/// 残す 1 つの選び方。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum KeepRule {
    /// 作成日時がいちばん古いもの（同じなら更新日時、パス）
    #[default]
    Oldest,
    /// いちばん浅いフォルダのもの
    Shallowest,
    /// 指定した目録（何番目）の中のもの（なければ `Oldest`）
    InRoot(usize),
}

/// 重複の検出の設定。
#[derive(Clone, Debug)]
pub struct DupeOptions {
    /// これより小さいファイルは対象にしない（既定 1。大きさ 0 は除く）
    pub min_size: u64,
    /// 全体のハッシュの後で、中身を 1 バイトずつ比べる
    pub byte_compare: bool,
    pub keep: KeepRule,
    /// 並列に読むファイルの数
    pub threads: usize,
}

impl Default for DupeOptions {
    fn default() -> Self {
        DupeOptions {
            min_size: 1,
            byte_compare: false,
            keep: KeepRule::Oldest,
            threads: 8,
        }
    }
}

/// 重複のグループ。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub size: u64,
    pub hash: Hash,
    /// 同じ中身のファイル（残すものが先頭）
    pub files: Vec<FileRef>,
}

impl Group {
    /// 消せば空く大きさ。
    pub fn wasted(&self) -> u64 {
        self.size * (self.files.len() as u64 - 1)
    }
}

/// 進み（読んだバイト数・調べたファイルの数・段階）。
#[derive(Clone, Copy, Debug, Default)]
pub struct DupeProgress {
    pub stage: u8,
    pub files: u64,
    pub bytes: u64,
}

fn entry(cats: &[Catalog], r: FileRef) -> &crate::scan::FileEntry {
    &cats[r.root].files[r.index]
}

/// ハッシュを（キャッシュになければ読んで）求める。段階 3 なら全体、2 なら先頭・末尾。
fn hashes(
    fs: &dyn Fs,
    cats: &[Catalog],
    indexes: &mut [Option<&mut Index>],
    refs: Vec<FileRef>,
    stage: u8,
    opts: &DupeOptions,
    progress: &(dyn Fn(&DupeProgress) -> bool + Sync),
) -> io::Result<Vec<(FileRef, Hash)>> {
    let full = stage == 3;
    let mut out = Vec::with_capacity(refs.len());
    let mut todo = Vec::new();
    for r in refs {
        let e = entry(cats, r);
        let cached = indexes
            .get(r.root)
            .and_then(|ix| ix.as_ref())
            .and_then(|ix| ix.lookup(&e.rel, &e.meta))
            .and_then(|c| if full { c.full } else { c.edges });
        match cached {
            Some(h) => out.push((r, h)),
            None => todo.push(r),
        }
    }
    let stop = AtomicBool::new(false);
    let files = AtomicU64::new(0);
    let bytes = AtomicU64::new(0);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(opts.threads.max(1))
        .build()
        .map_err(io::Error::other)?;
    let computed: Vec<(FileRef, Hash)> = pool.install(|| {
        todo.par_iter()
            .filter_map(|&r| {
                if stop.load(Ordering::Relaxed) {
                    return None;
                }
                let e = entry(cats, r);
                let path = cats[r.root].path(e);
                let h = if full {
                    hash::full(fs, &path, &mut |n| {
                        let b = bytes.fetch_add(n, Ordering::Relaxed) + n;
                        let ok = progress(&DupeProgress {
                            stage,
                            files: files.load(Ordering::Relaxed),
                            bytes: b,
                        });
                        if !ok {
                            stop.store(true, Ordering::Relaxed);
                        }
                        ok
                    })
                } else {
                    hash::edges(fs, &path, e.meta.size)
                };
                let f = files.fetch_add(1, Ordering::Relaxed) + 1;
                if !full
                    && !progress(&DupeProgress {
                        stage,
                        files: f,
                        bytes: bytes.load(Ordering::Relaxed),
                    })
                {
                    stop.store(true, Ordering::Relaxed);
                }
                match h {
                    Ok(h) => Some((r, h)),
                    Err(x) if crate::is_cancelled(&x) => None,
                    // 読めないファイル（消えた・使用中）は外す
                    Err(_) => None,
                }
            })
            .collect()
    });
    if stop.load(Ordering::Relaxed) {
        return Err(crate::cancelled());
    }
    for &(r, h) in &computed {
        let e = entry(cats, r);
        if let Some(Some(ix)) = indexes.get_mut(r.root) {
            let c = ix.entry(&e.rel, &e.meta);
            if full {
                c.full = Some(h);
            } else {
                c.edges = Some(h);
            }
        }
    }
    out.extend(computed);
    Ok(out)
}

/// 2 つのファイルの中身が同じか（1 バイトずつ）。
fn same_bytes(fs: &dyn Fs, a: &std::path::Path, b: &std::path::Path) -> io::Result<bool> {
    let mut fa = fs.open_read(a)?;
    let mut fb = fs.open_read(b)?;
    let mut ba = vec![0u8; 1 << 20];
    let mut bb = vec![0u8; 1 << 20];
    loop {
        let n = read_full(&mut *fa, &mut ba)?;
        let m = read_full(&mut *fb, &mut bb)?;
        if n != m || ba[..n] != bb[..m] {
            return Ok(false);
        }
        if n == 0 {
            return Ok(true);
        }
    }
}

fn read_full(r: &mut dyn Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}

/// 重複のグループを求める（消せば空く大きさの大きい順）。`indexes` は目録ごとの索引（なければ `None`）で、
/// 求めたハッシュを書き足す。
pub fn find(
    fs: &dyn Fs,
    cats: &[Catalog],
    indexes: &mut [Option<&mut Index>],
    opts: &DupeOptions,
    progress: &(dyn Fn(&DupeProgress) -> bool + Sync),
) -> io::Result<Vec<Group>> {
    // (1) 大きさ。同じファイル ID（ハードリンク）は 1 つにする
    let mut by_size: HashMap<u64, Vec<FileRef>> = HashMap::new();
    let mut seen_ids = std::collections::HashSet::new();
    for (ri, c) in cats.iter().enumerate() {
        for (fi, f) in c.files.iter().enumerate() {
            if f.meta.size < opts.min_size {
                continue;
            }
            if let Some(id) = f.meta.file_id
                && !seen_ids.insert(id)
            {
                continue;
            }
            by_size.entry(f.meta.size).or_default().push(FileRef {
                root: ri,
                index: fi,
            });
        }
    }
    let cand: Vec<FileRef> = by_size
        .into_values()
        .filter(|v| v.len() > 1)
        .flatten()
        .collect();
    // (2) 大きさと先頭・末尾。先頭と末尾で全体を読む小さなファイルは、これで決まる
    let edges = hashes(fs, cats, indexes, cand, 2, opts, progress)?;
    let mut by_edge: HashMap<(u64, Hash), Vec<FileRef>> = HashMap::new();
    for (r, h) in edges {
        by_edge
            .entry((entry(cats, r).meta.size, h))
            .or_default()
            .push(r);
    }
    let mut groups: Vec<Group> = Vec::new();
    let mut cand = Vec::new();
    for ((size, h), files) in by_edge {
        if files.len() < 2 {
            continue;
        }
        if size <= 2 * hash::EDGE {
            groups.push(Group {
                size,
                hash: h,
                files,
            });
        } else {
            cand.extend(files);
        }
    }
    // (3) 全体
    let full = hashes(fs, cats, indexes, cand, 3, opts, progress)?;
    let mut by_full: HashMap<(u64, Hash), Vec<FileRef>> = HashMap::new();
    for (r, h) in full {
        by_full
            .entry((entry(cats, r).meta.size, h))
            .or_default()
            .push(r);
    }
    for ((size, h), files) in by_full {
        if files.len() > 1 {
            groups.push(Group {
                size,
                hash: h,
                files,
            });
        }
    }
    // 1 バイトずつ比べる（任意）
    if opts.byte_compare {
        let mut checked = Vec::new();
        for g in groups {
            let mut rest = g.files.clone();
            while let Some(first) = rest.first().copied() {
                let a = cats[first.root].path(entry(cats, first));
                let (same, other): (Vec<FileRef>, Vec<FileRef>) =
                    rest.into_iter().partition(|&r| {
                        r == first
                            || same_bytes(fs, &a, &cats[r.root].path(entry(cats, r)))
                                .unwrap_or(false)
                    });
                if same.len() > 1 {
                    checked.push(Group {
                        size: g.size,
                        hash: g.hash,
                        files: same,
                    });
                }
                rest = other;
            }
        }
        groups = checked;
    }
    for g in &mut groups {
        order_keep(cats, g, &opts.keep);
    }
    groups.sort_by(|a, b| {
        b.wasted()
            .cmp(&a.wasted())
            .then_with(|| a.files[0].cmp(&b.files[0]))
    });
    Ok(groups)
}

/// 残すものを先頭にする（残りはパスの順）。
fn order_keep(cats: &[Catalog], g: &mut Group, rule: &KeepRule) {
    let key = |r: &FileRef| {
        let e = entry(cats, *r);
        let in_root = match rule {
            KeepRule::InRoot(i) => r.root != *i,
            _ => false,
        };
        let depth = e.rel.matches('/').count();
        let primary = match rule {
            KeepRule::Shallowest => depth as i64,
            _ => 0,
        };
        let created = if e.meta.ctime != 0 {
            e.meta.ctime
        } else {
            e.meta.mtime
        };
        (
            in_root,
            primary,
            created,
            e.meta.mtime,
            r.root,
            e.rel.clone(),
        )
    };
    g.files.sort_by_key(key);
}

/// 1 つのファイルと同じ中身のファイルを探す（18 章 8.4「同じ中身のファイルを探す」）。大きさが同じ
/// ものだけを読む。見つかれば、そのファイルを先頭にしたグループ。
pub fn same_content(
    fs: &dyn Fs,
    cats: &[Catalog],
    target: FileRef,
    cancel: &dyn Fn() -> bool,
) -> io::Result<Option<Group>> {
    let t = &cats[target.root].files[target.index];
    let size = t.meta.size;
    let tp = cats[target.root].path(t);
    let th = crate::hash::full(fs, &tp, &mut |_| !cancel())?;
    let mut files = vec![target];
    for (ri, c) in cats.iter().enumerate() {
        for (fi, f) in c.files.iter().enumerate() {
            let r = FileRef {
                root: ri,
                index: fi,
            };
            if r == target || f.meta.size != size || f.meta.link {
                continue;
            }
            // 同じファイル（ハードリンク・同じ場所を 2 回）は除く
            if f.meta.file_id.is_some() && f.meta.file_id == t.meta.file_id {
                continue;
            }
            if cancel() {
                return Err(crate::cancelled());
            }
            match crate::hash::full(fs, &c.path(f), &mut |_| !cancel()) {
                Ok(h) if h == th => files.push(r),
                Ok(_) => {}
                Err(e) if crate::is_cancelled(&e) => return Err(e),
                Err(_) => {} // 読めないものは飛ばす
            }
        }
    }
    Ok((files.len() > 1).then_some(Group {
        size,
        hash: th,
        files,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::Local;
    use crate::scan::{ScanOptions, scan};

    #[test]
    fn finds_files_with_the_same_content_as_one() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("a");
        for (rel, body) in [
            ("x.txt", "same body"),
            ("sub/y.txt", "same body"),
            ("z.txt", "same bodY"),
            ("w.txt", "other"),
        ] {
            let p = a.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        }
        let cats = vec![scan(&Local, &a, &ScanOptions::default(), &|_| true).unwrap()];
        let at = |rel: &str| FileRef {
            root: 0,
            index: cats[0].files.iter().position(|f| f.rel == rel).unwrap(),
        };
        let g = same_content(&Local, &cats, at("x.txt"), &|| false)
            .unwrap()
            .unwrap();
        assert_eq!(g.files, vec![at("x.txt"), at("sub/y.txt")]);
        assert!(
            same_content(&Local, &cats, at("w.txt"), &|| false)
                .unwrap()
                .is_none()
        );
        assert!(is_cancelled_err(same_content(
            &Local,
            &cats,
            at("x.txt"),
            &|| true
        )));
    }

    fn is_cancelled_err(r: io::Result<Option<Group>>) -> bool {
        r.is_err_and(|e| crate::is_cancelled(&e))
    }

    #[test]
    fn finds_exact_duplicates_in_three_stages() {
        let d = tempfile::tempdir().unwrap();
        let (a, b) = (d.path().join("a"), d.path().join("b"));
        let big: Vec<u8> = (0..400_000u32).map(|i| (i % 253) as u8).collect();
        let mut mid = big.clone();
        mid[200_000] ^= 0xff; // 先頭・末尾は同じで真ん中が違う
        for (p, body) in [
            (a.join("x/big.bin"), &big),
            (a.join("big copy.bin"), &big),
            (b.join("deep/er/big.bin"), &big),
            (a.join("near.bin"), &mid),
        ] {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        }
        for (p, body) in [
            (a.join("s1.txt"), &b"small"[..]),
            (b.join("s2.txt"), b"small"),
            (a.join("other.txt"), b"smalL"),
            (a.join("empty1"), b""),
            (a.join("empty2"), b""),
        ] {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        }
        #[cfg(unix)]
        std::fs::hard_link(a.join("s1.txt"), a.join("s1-link.txt")).unwrap();
        let cats = vec![
            scan(&Local, &a, &ScanOptions::default(), &|_| true).unwrap(),
            scan(&Local, &b, &ScanOptions::default(), &|_| true).unwrap(),
        ];
        let mut ia = Index::new(&a);
        let mut ib = Index::new(&b);
        let opts = DupeOptions {
            keep: KeepRule::Shallowest,
            ..DupeOptions::default()
        };
        let g = find(
            &Local,
            &cats,
            &mut [Some(&mut ia), Some(&mut ib)],
            &opts,
            &|_| true,
        )
        .unwrap();
        let names = |g: &Group| -> Vec<String> {
            g.files
                .iter()
                .map(|r| format!("{}:{}", r.root, cats[r.root].files[r.index].rel))
                .collect()
        };
        assert_eq!(g.len(), 2, "{g:?}");
        // 大きいものが先、残すもの（いちばん浅い）が先頭
        assert_eq!(
            names(&g[0]),
            ["0:big copy.bin", "0:x/big.bin", "1:deep/er/big.bin"]
        );
        assert_eq!(g[0].wasted(), 800_000);
        assert_eq!(g[0].hash, hash::bytes(&big));
        assert_eq!(names(&g[1]).len(), 2); // s1 と s2（ハードリンクは数えない・空は除く）
        // 2 回目はキャッシュから（読まない）
        let read = AtomicU64::new(0);
        let g2 = find(
            &Local,
            &cats,
            &mut [Some(&mut ia), Some(&mut ib)],
            &opts,
            &|p| {
                read.store(p.bytes, Ordering::Relaxed);
                true
            },
        )
        .unwrap();
        assert_eq!(g2, g);
        assert_eq!(read.load(Ordering::Relaxed), 0);
        // 索引なし・1 バイトずつ比べる・残すものを目録で選ぶ
        let opts = DupeOptions {
            byte_compare: true,
            keep: KeepRule::InRoot(1),
            ..DupeOptions::default()
        };
        let g3 = find(&Local, &cats, &mut [None, None], &opts, &|_| true).unwrap();
        assert_eq!(names(&g3[0])[0], "1:deep/er/big.bin");
        // 中止
        let mut ix = Index::new(&a);
        let e = find(
            &Local,
            &cats[..1],
            &mut [Some(&mut ix)],
            &DupeOptions::default(),
            &|_| false,
        )
        .unwrap_err();
        assert!(crate::is_cancelled(&e));
    }
}
