//! Three-way inventory comparison. Missing files remain distinct from empty files.
//! Directory equality refers only to the container; children have their own rows.
use super::{equal, is_link, scan};
use crate::{Cancellation, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeKind {
    File,
    Directory,
    Link,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    Unchanged,
    LeftChanged,
    RightChanged,
    SameChange,
    BothChanged,
    LeftAdded,
    RightAdded,
    SameAddition,
    AdditionConflict,
    LeftDeleted,
    RightDeleted,
    BothDeleted,
    DeleteModifyConflict,
    TypeConflict,
    SkippedLink,
    Error,
}
#[derive(Debug, Serialize)]
pub struct Entry {
    pub path: PathBuf,
    pub status: Status,
    pub base: Option<NodeKind>,
    pub left: Option<NodeKind>,
    pub right: Option<NodeKind>,
    pub detail: Option<String>,
}
impl Entry {
    pub fn incomplete(&self) -> bool {
        matches!(self.status, Status::SkippedLink | Status::Error)
    }
    pub fn file_pair(&self) -> bool {
        !self.incomplete()
            && self.left == Some(NodeKind::File)
            && self.right == Some(NodeKind::File)
    }
    pub fn three_files(&self) -> bool {
        self.file_pair() && self.base == Some(NodeKind::File)
    }
}
struct Tree {
    root: PathBuf,
    nodes: BTreeMap<PathBuf, NodeKind>,
    links: BTreeSet<PathBuf>,
}
impl Tree {
    fn scan(root: &Path, recursive: bool, cancel: &Cancellation) -> Result<Self> {
        let k = fs::symlink_metadata(root)
            .map_err(|e| format!("{}: {e}", root.display()))?
            .file_type();
        if !k.is_dir() || is_link(root, k)? {
            return Err("folder root must be a real directory".into());
        }
        let mut raw = BTreeMap::new();
        scan(root, Path::new(""), recursive, &mut raw, cancel)?;
        let mut tree = Self {
            root: root.to_owned(),
            nodes: BTreeMap::new(),
            links: BTreeSet::new(),
        };
        for (path, kind) in raw {
            cancel.check()?;
            let kind = if is_link(&root.join(&path), kind)? {
                tree.links.insert(path.clone());
                NodeKind::Link
            } else if kind.is_dir() {
                NodeKind::Directory
            } else {
                NodeKind::File
            };
            tree.nodes.insert(path, kind);
        }
        Ok(tree)
    }
    fn blocked(&self, path: &Path) -> bool {
        // A missing descendant below an untraversed link is unknown, not a deletion.
        !self.links.is_empty() && path.ancestors().any(|p| self.links.contains(p))
    }
}
fn same(a: &Tree, b: &Tree, path: &Path, cancel: &Cancellation) -> Result<bool> {
    match (a.nodes.get(path), b.nodes.get(path)) {
        (None, None) | (Some(NodeKind::Directory), Some(NodeKind::Directory)) => Ok(true),
        (Some(NodeKind::File), Some(NodeKind::File)) => {
            equal(&a.root.join(path), &b.root.join(path), cancel).map_err(|e| {
                format!(
                    "{} <> {}: {e}",
                    a.root.join(path).display(),
                    b.root.join(path).display()
                )
            })
        }
        _ => Ok(false),
    }
}
fn classify(kinds: [Option<NodeKind>; 3], bl: bool, br: bool, lr: bool) -> Status {
    use Status::*;
    let [b, l, r] = kinds;
    if bl && br {
        Unchanged
    } else if lr {
        if l.is_none() {
            BothDeleted
        } else if b.is_none() {
            SameAddition
        } else {
            SameChange
        }
    } else if bl {
        if r.is_none() {
            RightDeleted
        } else if b.is_none() {
            RightAdded
        } else {
            RightChanged
        }
    } else if br {
        if l.is_none() {
            LeftDeleted
        } else if b.is_none() {
            LeftAdded
        } else {
            LeftChanged
        }
    } else if l.is_none() || r.is_none() {
        DeleteModifyConflict
    } else if l != r {
        TypeConflict
    } else if b.is_none() {
        AdditionConflict
    } else {
        BothChanged
    }
}
pub fn compare(
    base: &Path,
    left: &Path,
    right: &Path,
    recursive: bool,
    cancel: &Cancellation,
) -> Result<Vec<Entry>> {
    let trees = [
        Tree::scan(base, recursive, cancel)?,
        Tree::scan(left, recursive, cancel)?,
        Tree::scan(right, recursive, cancel)?,
    ];
    compare_trees([&trees[0], &trees[1], &trees[2]], cancel)
}
/// Same inventory model without scanning/reading the left tree twice for directional plans.
pub(crate) fn compare_pair(left: &Path, right: &Path, cancel: &Cancellation) -> Result<Vec<Entry>> {
    let left = Tree::scan(left, true, cancel)?;
    let right = Tree::scan(right, true, cancel)?;
    compare_trees([&left, &left, &right], cancel)
}
fn compare_trees(trees: [&Tree; 3], cancel: &Cancellation) -> Result<Vec<Entry>> {
    let shared_ancestor = std::ptr::eq(trees[0], trees[1]);
    let mut paths = BTreeSet::new();
    for (i, tree) in trees.iter().enumerate() {
        if i == 0 && shared_ancestor {
            continue;
        }
        paths.extend(tree.nodes.keys().cloned());
    }
    let mut out = Vec::with_capacity(paths.len());
    for path in paths {
        cancel.check()?;
        let kinds = trees.each_ref().map(|t| t.nodes.get(&path).copied());
        let mut detail = None;
        let status = if trees.iter().any(|t| t.blocked(&path)) {
            Status::SkippedLink
        } else {
            let comparisons = (|| {
                let bl = if shared_ancestor {
                    true
                } else {
                    same(trees[0], trees[1], &path, cancel)?
                };
                let br = same(trees[0], trees[2], &path, cancel)?;
                let lr = if shared_ancestor {
                    br
                } else {
                    same(trees[1], trees[2], &path, cancel)?
                };
                if u8::from(bl) + u8::from(br) + u8::from(lr) == 2 {
                    return Err(
                        "content changed between comparisons; rerun the folder comparison".into(),
                    );
                }
                Ok((bl, br, lr))
            })();
            match comparisons {
                Ok((bl, br, lr)) => classify(kinds, bl, br, lr),
                Err(e) => {
                    cancel.check()?;
                    detail = Some(e);
                    Status::Error
                }
            }
        };
        out.push(Entry {
            path,
            status,
            base: kinds[0],
            left: kinds[1],
            right: kinds[2],
            detail,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    #[test]
    fn unreadable_member_is_incomplete_and_not_openable() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = tempfile::tempdir().unwrap();
        let roots = ["base", "left", "right"].map(|name| dir.path().join(name));
        for root in &roots {
            fs::create_dir(root).unwrap();
            fs::write(root.join("file"), "data").unwrap();
        }
        let _lock = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(roots[1].join("file"))
            .unwrap();
        let rows = compare(
            &roots[0],
            &roots[1],
            &roots[2],
            true,
            &Cancellation::default(),
        )
        .unwrap();
        assert_eq!(rows[0].status, Status::Error);
        assert!(rows[0].detail.is_some());
        assert!(!rows[0].file_pair());
        assert!(!rows[0].three_files());
    }
    #[test]
    fn exhaustive_presence_and_content_states_match_the_ancestor_contract() {
        let dir = tempfile::tempdir().unwrap();
        let roots = [
            dir.path().join("base"),
            dir.path().join("left"),
            dir.path().join("right"),
        ];
        for root in &roots {
            fs::create_dir(root).unwrap();
        }
        // None, empty, two different non-empty byte sequences: 64 complete cases.
        let states: [Option<&[u8]>; 4] = [None, Some(b""), Some(b"a"), Some(b"b")];
        for b in 0..4 {
            for l in 0..4 {
                for r in 0..4 {
                    for (root, i) in roots.iter().zip([b, l, r]) {
                        let path = root.join("file");
                        if let Some(bytes) = states[i] {
                            fs::write(path, bytes).unwrap();
                        } else if path.exists() {
                            fs::remove_file(path).unwrap();
                        }
                    }
                    let rows = compare(
                        &roots[0],
                        &roots[1],
                        &roots[2],
                        true,
                        &Cancellation::default(),
                    )
                    .unwrap();
                    if b == 0 && l == 0 && r == 0 {
                        assert!(rows.is_empty());
                        continue;
                    }
                    let row = &rows[0];
                    assert_eq!(
                        row.status == Status::Unchanged,
                        b == l && b == r,
                        "{b}/{l}/{r}"
                    );
                    assert_eq!(
                        [row.base.is_some(), row.left.is_some(), row.right.is_some()],
                        [b != 0, l != 0, r != 0]
                    );
                    assert_eq!(row.file_pair(), l != 0 && r != 0);
                    assert!(!row.incomplete());
                    assert_eq!(row.three_files(), b != 0 && l != 0 && r != 0);
                }
            }
        }
        // Named outcomes from the user-facing contract, including empty vs absent.
        for (indices, expected) in [
            ([0, 1, 0], Status::LeftAdded),
            ([0, 0, 1], Status::RightAdded),
            ([0, 1, 1], Status::SameAddition),
            ([0, 1, 2], Status::AdditionConflict),
            ([1, 0, 0], Status::BothDeleted),
            ([1, 0, 1], Status::LeftDeleted),
            ([1, 1, 0], Status::RightDeleted),
            ([1, 0, 2], Status::DeleteModifyConflict),
            ([1, 2, 0], Status::DeleteModifyConflict),
            ([1, 2, 1], Status::LeftChanged),
            ([1, 1, 2], Status::RightChanged),
            ([1, 2, 2], Status::SameChange),
            ([1, 2, 3], Status::BothChanged),
        ] {
            for (root, i) in roots.iter().zip(indices) {
                let path = root.join("file");
                if let Some(bytes) = states[i] {
                    fs::write(path, bytes).unwrap();
                } else if path.exists() {
                    fs::remove_file(path).unwrap();
                }
            }
            let rows = compare(
                &roots[0],
                &roots[1],
                &roots[2],
                true,
                &Cancellation::default(),
            )
            .unwrap();
            assert_eq!(rows[0].status, expected, "{indices:?}");
        }
    }
    #[test]
    fn disjoint_text_edits_are_needs_merge_not_a_claim_of_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let roots: Vec<_> = ["base", "left", "right"]
            .map(|name| dir.path().join(name))
            .into();
        for (root, content) in roots.iter().zip(["a\nb\n", "A\nb\n", "a\nB\n"]) {
            fs::create_dir(root).unwrap();
            fs::write(root.join("text"), content).unwrap();
        }
        assert_eq!(
            compare(
                &roots[0],
                &roots[1],
                &roots[2],
                true,
                &Cancellation::default()
            )
            .unwrap()[0]
                .status,
            Status::BothChanged
        );
        assert_eq!(
            crate::merge("a\nb\n", "A\nb\n", "a\nB\n", &Cancellation::default())
                .unwrap()
                .unresolved(),
            0
        );
        fs::remove_file(roots[2].join("text")).unwrap();
        fs::create_dir(roots[2].join("text")).unwrap();
        assert_eq!(
            compare(
                &roots[0],
                &roots[1],
                &roots[2],
                true,
                &Cancellation::default()
            )
            .unwrap()[0]
                .status,
            Status::TypeConflict
        );
        let cancel = Cancellation::default();
        cancel.cancel();
        assert!(compare(&roots[0], &roots[1], &roots[2], true, &cancel).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn descendants_under_a_link_are_unknown_instead_of_deletions() {
        let dir = tempfile::tempdir().unwrap();
        let roots = ["base", "left", "right"].map(|name| dir.path().join(name));
        for root in &roots {
            fs::create_dir(root).unwrap();
        }
        fs::create_dir(roots[0].join("nested")).unwrap();
        fs::write(roots[0].join("nested/file"), "base").unwrap();
        std::os::unix::fs::symlink(roots[0].join("nested"), roots[1].join("nested")).unwrap();
        let rows = compare(
            &roots[0],
            &roots[1],
            &roots[2],
            true,
            &Cancellation::default(),
        )
        .unwrap();
        assert!(rows.iter().all(|e| e.status == Status::SkippedLink));
        assert!(rows.iter().all(|e| !e.file_pair()));
    }
}
