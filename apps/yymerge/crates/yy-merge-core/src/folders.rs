//! Read-only recursive folder comparison. Links are reported, never traversed.
use crate::{Cancellation, Result};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
};
#[derive(Debug, Serialize)]
pub enum Status {
    Equal,
    Different,
    LeftOnly,
    RightOnly,
    SkippedLink,
    TypeMismatch,
}
#[derive(Debug, Serialize)]
pub struct Entry {
    pub path: PathBuf,
    pub status: Status,
}
fn scan(
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
fn is_link(path: &Path, kind: fs::FileType) -> Result<bool> {
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
            let status = if lk.is_some_and(|k| is_link(&left.join(&path), *k).unwrap_or(true))
                || rk.is_some_and(|k| is_link(&right.join(&path), *k).unwrap_or(true))
            {
                Status::SkippedLink
            } else {
                match (lk, rk) {
                    (Some(a), Some(b)) if a.is_dir() && b.is_dir() => Status::Equal,
                    (Some(a), Some(b)) if a.is_file() && b.is_file() => {
                        if equal(&left.join(&path), &right.join(&path), cancel)? {
                            Status::Equal
                        } else {
                            Status::Different
                        }
                    }
                    (Some(_), Some(_)) => Status::TypeMismatch,
                    (Some(_), None) => Status::LeftOnly,
                    (None, Some(_)) => Status::RightOnly,
                    _ => unreachable!(),
                }
            };
            Ok(Entry { path, status })
        })
        .collect()
}
