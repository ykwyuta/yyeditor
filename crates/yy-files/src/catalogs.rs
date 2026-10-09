//! 目録の保存と再利用（18 章 3・8.2）。
//!
//! 走査した目録をルートごとに保存しておき、検索・似たファイルでは、設定の時間（既定 60 分）より新しい
//! 目録があれば走査し直さずに使う。除くものの設定が違う目録は使わない。

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::fs::Fs;
use crate::scan::{Catalog, ScanOptions, ScanProgress, scan};

const MAGIC: &[u8; 8] = b"YYFMCAT1";

/// 保存した目録。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stored {
    /// 走査した日時（ナノ秒）
    pub scanned_at: i64,
    /// 走査したときの除くものの設定（違えば使わない）
    pub excludes: Vec<String>,
    pub catalog: Catalog,
}

fn excludes(o: &ScanOptions) -> Vec<String> {
    let mut v: Vec<String> = o.exclude_files.items().to_vec();
    v.push("|".into());
    v.extend(o.exclude_dirs.items().iter().cloned());
    v
}

/// 目録のファイルの置き場所。
pub fn path_for(dir: &Path, root: &Path) -> PathBuf {
    crate::index::path_for(dir, root).with_extension("cat")
}

pub fn load(dir: &Path, root: &Path) -> io::Result<Stored> {
    let raw = std::fs::read(path_for(dir, root))?;
    let body = raw
        .strip_prefix(MAGIC.as_slice())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "目録の形式が違います"))?;
    let plain = miniz_oxide::inflate::decompress_to_vec(body)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{e:?}")))?;
    postcard::from_bytes(&plain).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

pub fn save(dir: &Path, cat: &Catalog, opts: &ScanOptions, scanned_at: i64) -> io::Result<()> {
    let st = Stored {
        scanned_at,
        excludes: excludes(opts),
        catalog: cat.clone(),
    };
    let plain = postcard::to_allocvec(&st).map_err(io::Error::other)?;
    let packed = miniz_oxide::deflate::compress_to_vec(&plain, 3);
    crate::index::write_atomic(
        &path_for(dir, root_of(cat)),
        &[MAGIC.as_slice(), &packed].concat(),
    )
}

fn root_of(c: &Catalog) -> &Path {
    &c.root
}

/// 目録を得る: `max_age`（ナノ秒）より新しい保存した目録があればそれを、なければ走査して保存する。
/// `force` なら必ず走査する。保存した目録を使ったときは、その走査の日時を返す。
#[allow(clippy::too_many_arguments)]
pub fn scan_cached(
    fs: &dyn Fs,
    root: &Path,
    opts: &ScanOptions,
    dir: &Path,
    max_age: i64,
    now: i64,
    force: bool,
    progress: &(dyn Fn(&ScanProgress) -> bool + Sync),
) -> io::Result<(Catalog, Option<i64>)> {
    if !force
        && let Ok(st) = load(dir, root)
        && st.excludes == excludes(opts)
        && st.catalog.root == root
        && now - st.scanned_at <= max_age
        && now >= st.scanned_at
    {
        return Ok((st.catalog, Some(st.scanned_at)));
    }
    let c = scan(fs, root, opts, progress)?;
    // 保存できなくても検索は続ける
    let _ = save(dir, &c, opts, now);
    Ok((c, None))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::Local;

    #[test]
    fn reuses_fresh_catalogs() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("r");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.txt"), "a").unwrap();
        let store = d.path().join("cats");
        let o = ScanOptions::default();
        const MIN: i64 = 60_000_000_000;
        let (c, used) = scan_cached(
            &Local,
            &root,
            &o,
            &store,
            60 * MIN,
            1000 * MIN,
            false,
            &|_| true,
        )
        .unwrap();
        assert_eq!((c.files.len(), used), (1, None));
        std::fs::write(root.join("b.txt"), "b").unwrap();
        // 新しいうちは保存した目録（b.txt はまだない）
        let (c, used) = scan_cached(
            &Local,
            &root,
            &o,
            &store,
            60 * MIN,
            1010 * MIN,
            false,
            &|_| true,
        )
        .unwrap();
        assert_eq!((c.files.len(), used), (1, Some(1000 * MIN)));
        // 古くなった・走査し直すを選んだ・除くものが違うときは走査する
        let (c, used) = scan_cached(
            &Local,
            &root,
            &o,
            &store,
            60 * MIN,
            1100 * MIN,
            false,
            &|_| true,
        )
        .unwrap();
        assert_eq!((c.files.len(), used), (2, None));
        let (_, used) = scan_cached(
            &Local,
            &root,
            &o,
            &store,
            60 * MIN,
            1101 * MIN,
            true,
            &|_| true,
        )
        .unwrap();
        assert_eq!(used, None);
        let o2 = ScanOptions {
            exclude_files: crate::pattern::Patterns::new(&["*.txt"]),
            ..ScanOptions::default()
        };
        let (c, used) = scan_cached(
            &Local,
            &root,
            &o2,
            &store,
            60 * MIN,
            1102 * MIN,
            false,
            &|_| true,
        )
        .unwrap();
        assert_eq!((c.files.len(), used), (0, None));
        assert_eq!(load(&store, &root).unwrap().catalog, c);
    }
}
