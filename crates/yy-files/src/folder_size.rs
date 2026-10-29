//! Incremental logical sizes. Never follow links or read file contents.
use crate::{DirEntry, Fs};
use std::{
    io,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Size {
    pub bytes: u64,
    pub files: u64,
    pub errors: u64,
    pub skipped_links: u64,
    pub skipped_mounts: u64,
    pub excluded: bool,
    pub complete: bool,
}
impl Size {
    fn add(&mut self, other: Self) {
        if self.bytes.checked_add(other.bytes).is_none() {
            self.errors = self.errors.saturating_add(1);
        }
        self.bytes = self.bytes.saturating_add(other.bytes);
        self.files = self.files.saturating_add(other.files);
        self.errors = self.errors.saturating_add(other.errors);
        self.skipped_links = self.skipped_links.saturating_add(other.skipped_links);
        self.skipped_mounts = self.skipped_mounts.saturating_add(other.skipped_mounts);
    }
}
struct Frame {
    path: PathBuf,
    entries: std::vec::IntoIter<DirEntry>,
    size: Size,
}
fn frame(fs: &dyn Fs, path: PathBuf, cancelled: &impl Fn() -> bool) -> Frame {
    if fs.metadata(&path).is_ok_and(|m| m.link) {
        return Frame {
            path,
            entries: Vec::new().into_iter(),
            size: Size {
                skipped_links: 1,
                ..Size::default()
            },
        };
    }
    match fs.read_dir_with_errors(&path, cancelled) {
        Ok((entries, errors)) => Frame {
            path,
            entries: entries.into_iter(),
            size: Size {
                errors,
                ..Size::default()
            },
        },
        Err(_) => Frame {
            path,
            entries: Vec::new().into_iter(),
            size: Size {
                errors: 1,
                ..Size::default()
            },
        },
    }
}
fn checkpoint(cancelled: &impl Fn() -> bool) -> io::Result<()> {
    if cancelled() {
        Err(crate::cancelled())
    } else {
        Ok(())
    }
}

/// Emit completed descendant totals for reuse, plus periodic lower bounds for
/// the immediate child being scanned. Hidden entries count; links do not.
pub fn scan(
    fs: &dyn Fs,
    root: &Path,
    cancelled: &impl Fn() -> bool,
    update: impl FnMut(&Path, Size),
) -> io::Result<Size> {
    scan_with(fs, root, &|_| false, cancelled, update)
}

pub fn scan_with(
    fs: &dyn Fs,
    root: &Path,
    exclude: &impl Fn(&Path) -> bool,
    cancelled: &impl Fn() -> bool,
    mut update: impl FnMut(&Path, Size),
) -> io::Result<Size> {
    checkpoint(cancelled)?;
    let meta = fs.metadata(root)?;
    if !meta.dir || meta.link {
        return Err(io::Error::other(
            "サイズ集計の起点はリンクでないフォルダにしてください",
        ));
    }
    let (entries, errors) = fs.read_dir_with_errors(root, cancelled)?;
    let mut total = Size {
        errors,
        ..Size::default()
    };
    let mut children = Vec::new();
    for entry in entries {
        checkpoint(cancelled)?;
        if entry.meta.link {
            total.skipped_links += 1;
        } else if entry.meta.dir {
            let path = root.join(entry.name);
            if exclude(&path) {
                total.skipped_mounts += 1;
                update(
                    &path,
                    Size {
                        excluded: true,
                        complete: true,
                        skipped_mounts: 1,
                        ..Size::default()
                    },
                );
            } else {
                children.push(path);
            }
        } else {
            total.add(Size {
                bytes: entry.meta.size,
                files: 1,
                ..Size::default()
            });
        }
    }
    update(root, total);
    let mut last = Instant::now();
    for child in children {
        checkpoint(cancelled)?;
        let mut subtotal = Size::default();
        let mut stack = vec![frame(fs, child.clone(), cancelled)];
        while let Some(top) = stack.last_mut() {
            checkpoint(cancelled)?;
            if let Some(entry) = top.entries.next() {
                if entry.meta.link {
                    top.size.skipped_links += 1;
                    subtotal.skipped_links += 1;
                } else if entry.meta.dir {
                    let path = top.path.join(entry.name);
                    if exclude(&path) {
                        top.size.skipped_mounts += 1;
                        subtotal.skipped_mounts += 1;
                        update(
                            &path,
                            Size {
                                excluded: true,
                                complete: true,
                                skipped_mounts: 1,
                                ..Size::default()
                            },
                        );
                    } else {
                        stack.push(frame(fs, path, cancelled));
                    }
                } else {
                    let value = Size {
                        bytes: entry.meta.size,
                        files: 1,
                        ..Size::default()
                    };
                    top.size.add(value);
                    subtotal.add(value);
                }
            } else {
                let mut done = stack.pop().unwrap();
                done.size.complete = true;
                update(&done.path, done.size);
                if let Some(parent) = stack.last_mut() {
                    parent.size.add(done.size);
                } else {
                    total.add(done.size);
                }
            }
            if last.elapsed() >= Duration::from_millis(100) && !stack.is_empty() {
                // Directory read failures in unfinished branches are accounted
                // for by their completed ancestor; this remains a lower bound.
                update(&child, subtotal);
                let mut partial = total;
                partial.add(subtotal);
                update(root, partial);
                last = Instant::now();
            }
        }
        update(root, total);
    }
    total.complete = true;
    update(root, total);
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sums_nested_hidden_files_and_empty_folders_without_reading_contents() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("a/nested")).unwrap();
        std::fs::create_dir(dir.path().join("empty")).unwrap();
        std::fs::write(dir.path().join("direct"), [0; 5]).unwrap();
        std::fs::write(dir.path().join("a/.hidden"), [0; 7]).unwrap();
        std::fs::write(dir.path().join("a/nested/file"), [0; 11]).unwrap();
        let mut updates = std::collections::HashMap::new();
        let result = scan(&crate::Local, dir.path(), &|| false, |p, s| {
            updates.insert(p.to_owned(), s);
        })
        .unwrap();
        assert_eq!(result.bytes, 23);
        assert_eq!(result.files, 3);
        assert!(result.complete && result.errors == 0);
        assert_eq!(updates[&dir.path().join("a")].bytes, 18);
        assert_eq!(updates[&dir.path().join("empty")].bytes, 0);
        assert_eq!(updates[dir.path()], result);
        let error = scan(&crate::Local, dir.path(), &|| true, |_, _| {
            panic!("cancelled")
        })
        .unwrap_err();
        assert!(crate::is_cancelled(&error));
    }
    #[test]
    fn exclusions_and_disappeared_directories_are_reported_without_false_zero_success() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("gone")).unwrap();
        std::fs::create_dir(dir.path().join("virtual")).unwrap();
        std::fs::write(dir.path().join("virtual/ignored"), [0; 99]).unwrap();
        std::fs::write(dir.path().join("kept"), [0; 5]).unwrap();
        let mut removed = false;
        let mut excluded = false;
        let result = scan_with(
            &crate::Local,
            dir.path(),
            &|p| p.file_name().is_some_and(|n| n == "virtual"),
            &|| false,
            |path, size| {
                if path == dir.path() && !removed {
                    std::fs::remove_dir(dir.path().join("gone")).unwrap();
                    removed = true;
                }
                if path.file_name().is_some_and(|n| n == "virtual") {
                    excluded = size.excluded && size.complete;
                }
            },
        )
        .unwrap();
        assert_eq!(
            (result.bytes, result.errors, result.skipped_mounts),
            (5, 1, 1)
        );
        assert!(result.complete && excluded);
    }
    #[cfg(unix)]
    #[test]
    fn links_and_cycles_do_not_escape_or_duplicate_totals() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("file"), [0; 9]).unwrap();
        std::os::unix::fs::symlink(dir.path(), dir.path().join("loop")).unwrap();
        std::os::unix::fs::symlink(dir.path().join("file"), dir.path().join("file-link")).unwrap();
        let size = scan(&crate::Local, dir.path(), &|| false, |_, _| {}).unwrap();
        assert_eq!(size.bytes, 9);
        assert_eq!(size.skipped_links, 2);
    }
}
