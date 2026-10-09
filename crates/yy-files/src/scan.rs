//! フォルダの走査と目録（18 章 3）。
//!
//! フォルダ単位に並列に読む（共有フォルダは 1 回の往復が遅いので、並べて読むほど速い）。再解析点
//! （シンボリック リンク・ジャンクション）はたどらない。

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::fs::{Fs, Meta};
use crate::pattern::{DEFAULT_EXCLUDE_DIRS, DEFAULT_EXCLUDE_FILES, Patterns};

/// 走査の設定。
#[derive(Clone, Debug)]
pub struct ScanOptions {
    pub exclude_files: Patterns,
    pub exclude_dirs: Patterns,
    /// 並列に読むフォルダの数
    pub threads: usize,
}

impl Default for ScanOptions {
    fn default() -> Self {
        ScanOptions {
            exclude_files: Patterns::new(DEFAULT_EXCLUDE_FILES),
            exclude_dirs: Patterns::new(DEFAULT_EXCLUDE_DIRS),
            threads: 16,
        }
    }
}

/// 目録の 1 ファイル。
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FileEntry {
    /// ルートからの相対パス（`/` 区切り）
    pub rel: String,
    pub meta: Meta,
}

impl FileEntry {
    /// 名前（最後の部分）。
    pub fn name(&self) -> &str {
        self.rel.rsplit('/').next().unwrap_or(&self.rel)
    }

    /// フォルダの相対パス（ルートなら空）。
    pub fn dir(&self) -> &str {
        self.rel.rsplit_once('/').map_or("", |(d, _)| d)
    }
}

/// 走査の結果。
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Catalog {
    pub root: PathBuf,
    /// ファイル（相対パスの順）
    pub files: Vec<FileEntry>,
    /// フォルダ（相対パスの順。ルートは含まない）
    pub dirs: Vec<String>,
    /// 読めなかったフォルダ（相対パス・理由）
    pub errors: Vec<(String, String)>,
}

impl Catalog {
    /// 大きさの合計。
    pub fn total_size(&self) -> u64 {
        self.files.iter().map(|f| f.meta.size).sum()
    }

    /// 相対パスのファイル。
    pub fn get(&self, rel: &str) -> Option<&FileEntry> {
        self.files
            .binary_search_by(|f| f.rel.as_str().cmp(rel))
            .ok()
            .map(|i| &self.files[i])
    }

    /// ファイルの実際のパス。
    pub fn path(&self, f: &FileEntry) -> PathBuf {
        crate::join(&self.root, &f.rel)
    }
}

/// 走査の進み。
#[derive(Clone, Copy, Debug, Default)]
pub struct ScanProgress {
    pub files: u64,
    pub dirs: u64,
    pub bytes: u64,
}

struct Shared<'a> {
    fs: &'a dyn Fs,
    root: &'a Path,
    opts: &'a ScanOptions,
    files: Mutex<Vec<FileEntry>>,
    dirs: Mutex<Vec<String>>,
    errors: Mutex<Vec<(String, String)>>,
    nfiles: AtomicU64,
    ndirs: AtomicU64,
    bytes: AtomicU64,
    stop: AtomicBool,
    progress: &'a (dyn Fn(&ScanProgress) -> bool + Sync),
}

fn visit<'s>(s: &rayon::Scope<'s>, sh: &'s Shared<'s>, rel: String) {
    if sh.stop.load(Ordering::Relaxed) {
        return;
    }
    let entries = match sh.fs.read_dir(&crate::join(sh.root, &rel)) {
        Ok(e) => e,
        Err(e) => {
            sh.errors.lock().unwrap().push((rel, e.to_string()));
            return;
        }
    };
    let mut local = Vec::new();
    for e in entries {
        let child = if rel.is_empty() {
            e.name.clone()
        } else {
            format!("{rel}/{}", e.name)
        };
        if e.meta.link {
            continue;
        }
        if e.meta.dir {
            if sh.opts.exclude_dirs.matches(&e.name) {
                continue;
            }
            sh.dirs.lock().unwrap().push(child.clone());
            sh.ndirs.fetch_add(1, Ordering::Relaxed);
            s.spawn(move |s| visit(s, sh, child));
        } else {
            if sh.opts.exclude_files.matches(&e.name) {
                continue;
            }
            sh.bytes.fetch_add(e.meta.size, Ordering::Relaxed);
            local.push(FileEntry {
                rel: child,
                meta: e.meta,
            });
        }
    }
    let n = local.len() as u64;
    sh.files.lock().unwrap().extend(local);
    let files = sh.nfiles.fetch_add(n, Ordering::Relaxed) + n;
    let p = ScanProgress {
        files,
        dirs: sh.ndirs.load(Ordering::Relaxed),
        bytes: sh.bytes.load(Ordering::Relaxed),
    };
    if !(sh.progress)(&p) {
        sh.stop.store(true, Ordering::Relaxed);
    }
}

/// `root` の下を走査する。`progress` が `false` を返したら中止する（[`crate::cancelled`]）。
pub fn scan(
    fs: &dyn Fs,
    root: &Path,
    opts: &ScanOptions,
    progress: &(dyn Fn(&ScanProgress) -> bool + Sync),
) -> io::Result<Catalog> {
    let m = fs.metadata(root)?;
    if !m.dir {
        return Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            format!("{} はフォルダではありません", root.display()),
        ));
    }
    // ルートが読めなければ、走査の失敗にする
    fs.read_dir(root)?;
    let sh = Shared {
        fs,
        root,
        opts,
        files: Mutex::new(Vec::new()),
        dirs: Mutex::new(Vec::new()),
        errors: Mutex::new(Vec::new()),
        nfiles: AtomicU64::new(0),
        ndirs: AtomicU64::new(0),
        bytes: AtomicU64::new(0),
        stop: AtomicBool::new(false),
        progress,
    };
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(opts.threads.max(1))
        .build()
        .map_err(io::Error::other)?;
    pool.scope(|s| visit(s, &sh, String::new()));
    if sh.stop.load(Ordering::Relaxed) {
        return Err(crate::cancelled());
    }
    let mut files = sh.files.into_inner().unwrap();
    files.sort_unstable_by(|a, b| a.rel.cmp(&b.rel));
    let mut dirs = sh.dirs.into_inner().unwrap();
    dirs.sort_unstable();
    let mut errors = sh.errors.into_inner().unwrap();
    errors.sort();
    Ok(Catalog {
        root: root.to_path_buf(),
        files,
        dirs,
        errors,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::Local;

    pub(crate) fn tree(root: &Path) {
        for (p, body) in [
            ("a.txt", "a"),
            ("sub/b.txt", "bb"),
            ("sub/深い/c.txt", "ccc"),
            ("sub/Thumbs.db", "x"),
            (".git/config", "x"),
            ("~$doc.docx", "x"),
        ] {
            let f = root.join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, body).unwrap();
        }
    }

    #[test]
    fn scans_in_parallel_and_excludes() {
        let d = tempfile::tempdir().unwrap();
        tree(d.path());
        #[cfg(unix)]
        std::os::unix::fs::symlink(d.path().join("sub"), d.path().join("loop")).unwrap();
        let c = scan(&Local, d.path(), &ScanOptions::default(), &|_| true).unwrap();
        let rels: Vec<&str> = c.files.iter().map(|f| f.rel.as_str()).collect();
        assert_eq!(rels, ["a.txt", "sub/b.txt", "sub/深い/c.txt"]);
        assert_eq!(c.dirs, ["sub", "sub/深い"]);
        assert_eq!(c.total_size(), 6);
        assert_eq!(c.get("sub/b.txt").unwrap().name(), "b.txt");
        assert_eq!(c.get("sub/深い/c.txt").unwrap().dir(), "sub/深い");
        assert!(c.get("nope").is_none());
        // 中止
        let e = scan(&Local, d.path(), &ScanOptions::default(), &|_| false).unwrap_err();
        assert!(crate::is_cancelled(&e));
        // フォルダでない
        assert!(
            scan(
                &Local,
                &d.path().join("a.txt"),
                &ScanOptions::default(),
                &|_| true
            )
            .is_err()
        );
    }
}
