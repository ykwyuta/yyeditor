//! Read-only recursive folder comparison. Links are reported, never traversed.
use crate::{Cancellation, Result};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
};
pub mod filter;
pub mod three;
#[derive(Debug, PartialEq, Eq, Serialize)]
pub enum Status {
    Equal,
    Different,
    LeftOnly,
    RightOnly,
    SkippedLink,
    TypeMismatch,
    Error,
}
#[derive(Debug, Serialize)]
pub struct Entry {
    pub path: PathBuf,
    pub status: Status,
    /// Both sides were regular files at comparison time. Directories are never opened as text.
    pub file_pair: bool,
    pub directory: bool,
    pub detail: Option<String>,
}
pub(super) fn scan(
    root: &Path,
    relative: &Path,
    recursive: bool,
    out: &mut BTreeMap<PathBuf, fs::FileType>,
    cancel: &Cancellation,
) -> Result<()> {
    cancel.check()?;
    if relative.components().count() > 128 {
        return Err("folder comparison incomplete: directory depth limit".into());
    }
    for entry in fs::read_dir(root.join(relative))
        .map_err(|e| format!("{}: {e}", root.join(relative).display()))?
    {
        cancel.check()?;
        let entry = entry.map_err(|e| e.to_string())?;
        let path = relative.join(entry.file_name());
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        if !kind.is_file() && !kind.is_dir() && !kind.is_symlink() {
            return Err(format!(
                "unsupported special file: {}",
                entry.path().display()
            ));
        }
        out.insert(path.clone(), kind);
        if out.len() > 100_000 {
            return Err("folder comparison incomplete: entry limit".into());
        }
        // Windows reparse points (junctions included) must not be followed.
        if recursive && kind.is_dir() && !is_link(&entry.path(), kind)? {
            scan(root, &path, true, out, cancel)?;
        }
    }
    Ok(())
}
pub(crate) fn is_link(path: &Path, kind: fs::FileType) -> Result<bool> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        Ok(kind.is_symlink()
            || fs::symlink_metadata(path)
                .map_err(|e| e.to_string())?
                .file_attributes()
                & 0x400
                != 0)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Ok(kind.is_symlink())
    }
}
fn equal(a: &Path, b: &Path, cancel: &Cancellation) -> Result<bool> {
    let mut a = fs::File::open(a).map_err(|e| e.to_string())?;
    let mut b = fs::File::open(b).map_err(|e| e.to_string())?;
    let before_a = a.metadata().map_err(|e| e.to_string())?;
    let before_b = b.metadata().map_err(|e| e.to_string())?;
    if before_a.len() != before_b.len() {
        return Ok(false);
    }
    let mut aa = [0; 65536];
    let mut bb = [0; 65536];
    let mut same = true;
    let mut remaining = before_a.len();
    while remaining > 0 {
        cancel.check()?;
        let n = remaining.min(aa.len() as u64) as usize;
        a.read_exact(&mut aa[..n]).map_err(|e| e.to_string())?;
        b.read_exact(&mut bb[..n]).map_err(|e| e.to_string())?;
        if aa[..n] != bb[..n] {
            same = false;
            break;
        }
        remaining -= n as u64;
    }
    for (before, file) in [(&before_a, &a), (&before_b, &b)] {
        let after = file.metadata().map_err(|e| e.to_string())?;
        if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
            return Err("file changed while comparing folders".into());
        }
    }
    Ok(same)
}
pub fn compare(
    left: &Path,
    right: &Path,
    recursive: bool,
    cancel: &Cancellation,
) -> Result<Vec<Entry>> {
    let mut l = BTreeMap::new();
    let mut r = BTreeMap::new();
    for root in [left, right] {
        let k = fs::symlink_metadata(root)
            .map_err(|e| e.to_string())?
            .file_type();
        if !k.is_dir() || is_link(root, k)? {
            return Err("folder root must be a real directory".into());
        }
    }
    scan(left, Path::new(""), recursive, &mut l, cancel)?;
    scan(right, Path::new(""), recursive, &mut r, cancel)?;
    let mut paths: Vec<_> = l.keys().chain(r.keys()).cloned().collect();
    paths.sort();
    paths.dedup();
    paths
        .into_iter()
        .map(|path| {
            cancel.check()?;
            let lk = l.get(&path);
            let rk = r.get(&path);
            let mut detail = None;
            let status = if lk.is_some_and(|k| is_link(&left.join(&path), *k).unwrap_or(true))
                || rk.is_some_and(|k| is_link(&right.join(&path), *k).unwrap_or(true))
            {
                Status::SkippedLink
            } else {
                match (lk, rk) {
                    (Some(a), Some(b)) if a.is_dir() && b.is_dir() => Status::Equal,
                    (Some(a), Some(b)) if a.is_file() && b.is_file() => {
                        match equal(&left.join(&path), &right.join(&path), cancel) {
                            Ok(true) => Status::Equal,
                            Ok(false) => Status::Different,
                            Err(e) => {
                                cancel.check()?;
                                detail = Some(e);
                                Status::Error
                            }
                        }
                    }
                    (Some(_), Some(_)) => Status::TypeMismatch,
                    (Some(_), None) => Status::LeftOnly,
                    (None, Some(_)) => Status::RightOnly,
                    _ => unreachable!(),
                }
            };
            let file_pair = lk.is_some_and(|k| k.is_file())
                && rk.is_some_and(|k| k.is_file())
                && !matches!(status, Status::SkippedLink | Status::Error);
            let directory = lk.is_some_and(|k| k.is_dir()) || rk.is_some_and(|k| k.is_dir());
            Ok(Entry {
                path,
                status,
                file_pair,
                directory,
                detail,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let l = d.path().join("left");
        let r = d.path().join("right");
        for root in [&l, &r] {
            fs::create_dir_all(root.join("nested")).unwrap();
        }
        (d, l, r)
    }
    #[test]
    fn recursive_results_preserve_file_pairs_and_directory_meaning() {
        let (_d, l, r) = fixture();
        for root in [&l, &r] {
            fs::write(root.join("equal"), [0, 255, 1]).unwrap();
        }
        fs::write(l.join("nested/日本語.txt"), "left").unwrap();
        fs::write(r.join("nested/日本語.txt"), "rght").unwrap();
        fs::write(l.join("only"), "left").unwrap();
        fs::write(r.join("other"), "right").unwrap();
        fs::create_dir(l.join("mismatch")).unwrap();
        fs::write(r.join("mismatch"), "file").unwrap();
        let rows = compare(&l, &r, true, &Cancellation::default()).unwrap();
        let row = |path| rows.iter().find(|e| e.path == Path::new(path)).unwrap();
        assert_eq!(row("equal").status, Status::Equal);
        assert!(row("equal").file_pair);
        assert_eq!(row("nested/日本語.txt").status, Status::Different);
        assert!(row("nested/日本語.txt").file_pair);
        assert_eq!(row("only").status, Status::LeftOnly);
        assert!(!row("only").file_pair);
        assert_eq!(row("other").status, Status::RightOnly);
        assert!(row("nested").directory);
        assert!(!row("nested").file_pair);
        assert_eq!(row("mismatch").status, Status::TypeMismatch);
        assert!(!row("mismatch").file_pair);
        assert_eq!(
            compare(&l, &r, false, &Cancellation::default())
                .unwrap()
                .len(),
            rows.len() - 1
        );
    }
    #[test]
    fn cancelled_or_missing_tree_never_returns_an_empty_success() {
        let (_d, l, r) = fixture();
        let cancel = Cancellation::default();
        cancel.cancel();
        assert!(compare(&l, &r, true, &cancel).is_err());
        assert!(compare(&l, &r.join("missing"), true, &Cancellation::default()).is_err());
    }
    #[cfg(windows)]
    #[test]
    fn locked_file_is_incomplete_and_other_rows_remain_available() {
        use std::os::windows::fs::OpenOptionsExt;
        let (_d, l, r) = fixture();
        for root in [&l, &r] {
            fs::write(root.join("locked"), "secret").unwrap();
            fs::write(root.join("readable"), "ok").unwrap();
        }
        let _lock = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(l.join("locked"))
            .unwrap();
        let rows = compare(&l, &r, true, &Cancellation::default()).unwrap();
        let error = rows.iter().find(|e| e.path == Path::new("locked")).unwrap();
        assert_eq!(error.status, Status::Error);
        assert!(error.detail.is_some());
        assert!(!error.file_pair);
        assert!(
            rows.iter()
                .any(|e| e.path == Path::new("readable") && e.status == Status::Equal)
        );
    }
    #[cfg(unix)]
    #[test]
    fn links_are_explicitly_uncompared_and_never_traversed() {
        use std::os::unix::fs::symlink;
        let (_d, l, r) = fixture();
        symlink(&l, l.join("cycle")).unwrap();
        let rows = compare(&l, &r, true, &Cancellation::default()).unwrap();
        let link = rows.iter().find(|e| e.path == Path::new("cycle")).unwrap();
        assert_eq!(link.status, Status::SkippedLink);
        assert!(!link.file_pair);
        assert!(compare(&l.join("cycle"), &r, true, &Cancellation::default()).is_err());
    }
}
