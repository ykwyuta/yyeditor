//! Synchronization proposals; execution requires a separate explicit command.
//! Exact inventory comparison is shared with folder comparison; no input is written.
use crate::{
    Cancellation, Result,
    folders::three::{self, NodeKind, Status},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    UpdateRight,
    UpdateLeft,
    MirrorRight,
    MirrorLeft,
    Merge,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Side {
    Left,
    Right,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Action {
    CreateDirectory,
    CreateFile,
    ReplaceFile,
    DeleteFile,
    RemoveDirectory,
    Keep,
    NeedsMerge,
    Conflict,
    Uncompared,
}
impl Action {
    pub fn operation(self) -> bool {
        matches!(
            self,
            Self::CreateDirectory
                | Self::CreateFile
                | Self::ReplaceFile
                | Self::DeleteFile
                | Self::RemoveDirectory
        )
    }
    pub fn review(self) -> bool {
        matches!(self, Self::NeedsMerge | Self::Conflict)
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::CreateDirectory => "フォルダー作成候補",
            Self::CreateFile => "新規コピー候補",
            Self::ReplaceFile => "置換候補",
            Self::DeleteFile => "ファイル削除候補",
            Self::RemoveDirectory => "フォルダー削除候補",
            Self::Keep => "維持",
            Self::NeedsMerge => "内容のマージが必要",
            Self::Conflict => "要確認: 衝突",
            Self::Uncompared => "未比較",
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Item {
    pub path: PathBuf,
    pub action: Action,
    pub source: Option<PathBuf>,
    pub destination: Option<PathBuf>,
    pub direction: Option<Side>,
    pub comparison: Option<Status>,
    pub kinds: [Option<NodeKind>; 3],
    pub reason: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Summary {
    pub operations: usize,
    pub review: usize,
    pub uncompared: usize,
    pub kept: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub version: u32,
    pub preview_only: bool,
    pub mode: Mode,
    pub base: Option<PathBuf>,
    pub left: PathBuf,
    pub right: PathBuf,
    pub items: Vec<Item>,
    pub summary: Summary,
    pub fingerprints: Option<crate::sync_run::Inventory>,
}
impl Plan {
    pub fn exit_code(&self) -> i32 {
        if self.summary.uncompared > 0 {
            2
        } else {
            i32::from(self.summary.review > 0)
        }
    }
    pub fn report(&self) -> Result<String> {
        serde_json::to_string_pretty(self)
            .map(|s| s + "\n")
            .map_err(|e| e.to_string())
    }
    pub fn validate_report_path(&self, path: &Path) -> Result<()> {
        let parent = fs::canonicalize(
            path.parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new(".")),
        )
        .map_err(|e| e.to_string())?;
        for root in [&self.left, &self.right]
            .into_iter()
            .chain(self.base.as_ref())
        {
            if parent.starts_with(root) {
                return Err("plan report must be outside all input trees".into());
            }
        }
        Ok(())
    }
}

fn direction(mode: Mode) -> (Side, Side, bool) {
    match mode {
        Mode::UpdateLeft => (Side::Right, Side::Left, false),
        Mode::MirrorLeft => (Side::Right, Side::Left, true),
        Mode::MirrorRight => (Side::Left, Side::Right, true),
        _ => (Side::Left, Side::Right, false),
    }
}
fn kind(e: &three::Entry, side: Side) -> Option<NodeKind> {
    if side == Side::Left { e.left } else { e.right }
}
fn copy_action(src: Option<NodeKind>, dst: Option<NodeKind>) -> Action {
    match (src, dst) {
        (Some(NodeKind::File), None) => Action::CreateFile,
        (Some(NodeKind::File), Some(NodeKind::File)) => Action::ReplaceFile,
        (Some(NodeKind::Directory), None) => Action::CreateDirectory,
        (Some(NodeKind::Directory), Some(NodeKind::Directory)) => Action::Keep,
        _ => Action::Conflict,
    }
}
fn delete_action(dst: Option<NodeKind>) -> Action {
    match dst {
        Some(NodeKind::File) => Action::DeleteFile,
        Some(NodeKind::Directory) => Action::RemoveDirectory,
        _ => Action::Conflict,
    }
}
fn decide(e: &three::Entry, mode: Mode) -> (Action, Option<Side>, Option<Side>, &'static str) {
    use Action as A;
    use Status as S;
    if e.incomplete() {
        return (
            A::Uncompared,
            None,
            None,
            "リンク・読み取り失敗のため操作を決められません",
        );
    }
    if mode != Mode::Merge {
        let (src, dst, mirror) = direction(mode);
        let (sk, dk) = (kind(e, src), kind(e, dst));
        if sk.is_none() {
            return if mirror {
                (
                    delete_action(dk),
                    None,
                    Some(dst),
                    "ミラー: 送り元にない項目",
                )
            } else {
                (
                    A::Keep,
                    None,
                    None,
                    "追加・更新のみ: 送り先だけの項目を維持",
                )
            };
        }
        if e.status == S::Unchanged {
            return (A::Keep, None, None, "内容・種類が同じ");
        }
        return (
            copy_action(sk, dk),
            Some(src),
            Some(dst),
            "指定した方向へ追加・更新（祖先による競合判定なし）",
        );
    }
    match e.status {
        S::Unchanged | S::SameChange | S::SameAddition | S::BothDeleted => {
            (A::Keep, None, None, "左右が同じ状態のため変更しません")
        }
        S::LeftAdded | S::LeftChanged => (
            copy_action(e.left, e.right),
            Some(Side::Left),
            Some(Side::Right),
            "左だけの変更を右へ反映する候補",
        ),
        S::RightAdded | S::RightChanged => (
            copy_action(e.right, e.left),
            Some(Side::Right),
            Some(Side::Left),
            "右だけの変更を左へ反映する候補",
        ),
        S::LeftDeleted => (
            delete_action(e.right),
            None,
            Some(Side::Right),
            "左で削除・右は祖先のまま",
        ),
        S::RightDeleted => (
            delete_action(e.left),
            None,
            Some(Side::Left),
            "右で削除・左は祖先のまま",
        ),
        S::BothChanged if e.three_files() => (
            A::NeedsMerge,
            None,
            None,
            "両側変更: ファイルマージで内容を確認してください",
        ),
        _ => (
            A::Conflict,
            None,
            None,
            "追加・削除と変更・種類変更が衝突しています",
        ),
    }
}
fn validate_roots(
    left: &Path,
    right: &Path,
    base: Option<&Path>,
    mode: Mode,
) -> Result<(PathBuf, PathBuf, Option<PathBuf>)> {
    if (mode == Mode::Merge) != base.is_some() {
        return Err("merge mode requires --base; directional modes do not accept --base".into());
    }
    let l = fs::canonicalize(left).map_err(|e| e.to_string())?;
    let r = fs::canonicalize(right).map_err(|e| e.to_string())?;
    let b = base
        .map(fs::canonicalize)
        .transpose()
        .map_err(|e| e.to_string())?;
    let roots = [&l, &r].into_iter().chain(b.as_ref()).collect::<Vec<_>>();
    for (i, a) in roots.iter().enumerate() {
        for other in roots.iter().skip(i + 1) {
            if a.starts_with(other) || other.starts_with(a) {
                return Err(
                    "synchronization plan roots must be distinct and non-overlapping".into(),
                );
            }
        }
    }
    Ok((l, r, b))
}
/// Recompare all roots; do not derive a plan from an old GUI result.
pub fn build(
    left: &Path,
    right: &Path,
    base: Option<&Path>,
    mode: Mode,
    cancel: &Cancellation,
) -> Result<Plan> {
    cancel.check()?;
    let (l, r, b) = validate_roots(left, right, base, mode)?;
    // Use original roots for reparse-point validation, canonical roots only for reporting.
    let entries = if let Some(base) = base {
        three::compare(base, left, right, true, cancel)?
    } else {
        three::compare_pair(left, right, cancel)?
    };
    let root = |side| if side == Side::Left { &l } else { &r };
    let mut items = Vec::with_capacity(entries.len());
    for e in entries {
        cancel.check()?;
        let (action, src, dst, reason) = decide(&e, mode);
        let reason = if action == Action::Conflict && src.is_some() {
            "ファイルとフォルダーの種類が異なるため置換しません".to_owned()
        } else {
            e.detail.clone().unwrap_or_else(|| reason.into())
        };
        items.push(Item {
            source: src.map(|s| root(s).join(&e.path)),
            destination: dst.map(|s| root(s).join(&e.path)),
            direction: dst,
            action,
            comparison: base.map(|_| e.status),
            kinds: [base.and(e.base), e.left, e.right],
            path: e.path,
            reason,
        });
    }
    // Conservatively flag portable Windows-name collisions, even when scanned on Unix.
    let mut names = BTreeMap::<String, Vec<usize>>::new();
    for (i, item) in items.iter_mut().enumerate() {
        cancel.check()?;
        if let Some(name) = item.path.to_str() {
            names.entry(name.to_lowercase()).or_default().push(i);
        } else {
            item.action = Action::Uncompared;
            item.reason = "Unicodeで表せないパスは計画対象にできません".into();
        }
    }
    for indices in names.values().filter(|v| v.len() > 1) {
        for &i in indices {
            if items[i].action != Action::Uncompared {
                items[i].action = Action::Conflict;
                items[i].reason = "大文字小文字だけ異なるパスが衝突しています".into();
            }
        }
    }
    // Never remove a container when any descendant is kept, unresolved or unexamined.
    let mut protected = BTreeSet::new();
    for item in &items {
        cancel.check()?;
        for (side, k) in [(Side::Left, item.kinds[1]), (Side::Right, item.kinds[2])] {
            if k.is_some()
                && !(matches!(item.action, Action::DeleteFile | Action::RemoveDirectory)
                    && item.direction == Some(side))
            {
                protected.extend(item.path.ancestors().skip(1).map(|p| (side, p.to_owned())));
            }
        }
    }
    for item in &mut items {
        if item.action == Action::RemoveDirectory
            && item
                .direction
                .is_some_and(|s| protected.contains(&(s, item.path.clone())))
        {
            item.action = Action::Conflict;
            item.reason = "配下に維持・変更・未比較の項目があるためフォルダーを削除しません".into();
        }
    }
    let blocked: BTreeSet<_> = items
        .iter()
        .filter(|i| matches!(i.action, Action::Conflict | Action::Uncompared))
        .map(|i| i.path.clone())
        .collect();
    for item in &mut items {
        cancel.check()?;
        if item.action.operation() && item.path.ancestors().skip(1).any(|p| blocked.contains(p)) {
            item.action = Action::Conflict;
            item.reason = "親パスが未解決のため操作を提案できません".into();
        }
    }
    // Parent creation first; children deleted before parents. No execution is performed.
    let order = |i: &Item| {
        let depth = i.path.components().count();
        let (phase, order) = match i.action {
            Action::CreateDirectory => (0, depth),
            Action::CreateFile | Action::ReplaceFile => (1, depth),
            Action::DeleteFile => (2, 0),
            Action::RemoveDirectory => (3, usize::MAX - depth),
            _ => (4, 0),
        };
        (phase, order)
    };
    items.sort_by(|a, b| order(a).cmp(&order(b)).then_with(|| a.path.cmp(&b.path)));
    let mut summary = Summary::default();
    for item in &items {
        if item.action.operation() {
            summary.operations += 1;
        } else if item.action.review() {
            summary.review += 1;
        } else if item.action == Action::Uncompared {
            summary.uncompared += 1;
        } else {
            summary.kept += 1;
        }
    }
    cancel.check()?;
    let mut plan = Plan {
        version: 2,
        preview_only: true,
        mode,
        base: b,
        left: l,
        right: r,
        items,
        summary,
        fingerprints: None,
    };
    if plan.summary.review == 0 && plan.summary.uncompared == 0 {
        plan.fingerprints = Some(crate::sync_run::capture(&plan, cancel)?);
    }
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn roots() -> (tempfile::TempDir, [PathBuf; 3]) {
        let dir = tempfile::tempdir().unwrap();
        let roots = ["base", "left", "right"].map(|name| dir.path().join(name));
        for root in &roots {
            fs::create_dir(root).unwrap();
        }
        (dir, roots)
    }
    fn write(root: &Path, path: &str, data: &str) {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, data).unwrap();
    }
    fn item<'a>(plan: &'a Plan, path: &str) -> &'a Item {
        plan.items
            .iter()
            .find(|i| i.path == Path::new(path))
            .unwrap()
    }
    #[test]
    fn update_and_mirror_directions_are_read_only_and_order_dependencies() {
        let (_dir, [_, left, right]) = roots();
        write(&left, "new/child", "new");
        write(&left, "changed", "left");
        write(&right, "changed", "right");
        write(&right, "obsolete/child", "keep until execution");
        fs::create_dir(right.join("empty")).unwrap();
        let c = Cancellation::default();
        let update = build(&left, &right, None, Mode::UpdateRight, &c).unwrap();
        assert_eq!(item(&update, "obsolete/child").action, Action::Keep);
        let plan = build(&left, &right, None, Mode::MirrorRight, &c).unwrap();
        assert_eq!(item(&plan, "new").action, Action::CreateDirectory);
        assert_eq!(item(&plan, "new/child").action, Action::CreateFile);
        assert_eq!(item(&plan, "changed").action, Action::ReplaceFile);
        assert_eq!(item(&plan, "obsolete/child").action, Action::DeleteFile);
        assert_eq!(item(&plan, "obsolete").action, Action::RemoveDirectory);
        assert_eq!(item(&plan, "empty").action, Action::RemoveDirectory);
        let position = |name| {
            plan.items
                .iter()
                .position(|i| i.path == Path::new(name))
                .unwrap()
        };
        assert!(position("new") < position("new/child"));
        assert!(position("obsolete/child") < position("obsolete"));
        assert!(plan.preview_only);
        assert_eq!(plan.exit_code(), 0);
        let reverse = build(&left, &right, None, Mode::MirrorLeft, &c).unwrap();
        assert_eq!(item(&reverse, "new/child").action, Action::DeleteFile);
        assert_eq!(item(&reverse, "obsolete/child").direction, Some(Side::Left));
        assert_eq!(fs::read_to_string(right.join("changed")).unwrap(), "right");
        assert!(right.join("obsolete/child").exists());
        assert!(!right.join("new").exists());
        assert_eq!(fs::read_to_string(left.join("changed")).unwrap(), "left");
    }
    #[test]
    fn ancestor_merge_keeps_conflicts_and_blocks_container_deletion() {
        let (_dir, [base, left, right]) = roots();
        for root in [&base, &left, &right] {
            write(root, "both", "base");
        }
        write(&left, "both", "left");
        write(&right, "both", "right");
        write(&base, "removed/changed", "old");
        write(&right, "removed/changed", "new");
        write(&base, "empty-now/gone", "old");
        fs::create_dir(right.join("empty-now")).unwrap();
        write(&base, "only-left", "base");
        write(&left, "only-left", "new");
        write(&right, "only-left", "base");
        write(&base, "only-right", "base");
        write(&left, "only-right", "base");
        write(&right, "only-right", "new");
        let plan = build(
            &left,
            &right,
            Some(&base),
            Mode::Merge,
            &Cancellation::default(),
        )
        .unwrap();
        assert_eq!(item(&plan, "both").action, Action::NeedsMerge);
        assert_eq!(item(&plan, "removed").action, Action::Conflict);
        assert_eq!(item(&plan, "removed/changed").action, Action::Conflict);
        // A child already absent on the destination must not protect an empty directory.
        assert_eq!(item(&plan, "empty-now").action, Action::RemoveDirectory);
        assert_eq!(item(&plan, "only-left").direction, Some(Side::Right));
        assert_eq!(item(&plan, "only-right").direction, Some(Side::Left));
        assert_eq!(plan.exit_code(), 1);
        assert!(right.join("removed/changed").exists());
        assert!(!left.join("removed").exists());
    }
    #[test]
    fn roots_reports_and_mode_contracts_are_validated_and_each_build_recompares() {
        let (dir, [base, left, right]) = roots();
        let c = Cancellation::default();
        write(&left, "file", "left");
        write(&right, "file", "right");
        assert_eq!(
            item(
                &build(&left, &right, None, Mode::UpdateRight, &c).unwrap(),
                "file"
            )
            .action,
            Action::ReplaceFile
        );
        write(&left, "file", "right");
        let plan = build(&left, &right, None, Mode::UpdateRight, &c).unwrap();
        assert_eq!(item(&plan, "file").action, Action::Keep);
        assert!(
            plan.validate_report_path(&left.join("report.json"))
                .is_err()
        );
        assert!(
            plan.validate_report_path(&dir.path().join("report.json"))
                .is_ok()
        );
        assert!(build(&left, &left, None, Mode::UpdateRight, &c).is_err());
        assert!(build(&left, &right, Some(&base), Mode::UpdateRight, &c).is_err());
        assert!(build(&left, &right, None, Mode::Merge, &c).is_err());
        fs::create_dir(left.join("nested")).unwrap();
        assert!(build(&left, &left.join("nested"), None, Mode::MirrorRight, &c).is_err());
        c.cancel();
        assert!(build(&left, &right, None, Mode::MirrorRight, &c).is_err());
    }
    #[test]
    fn case_collisions_and_type_conflicts_block_dependent_operations() {
        let (_dir, [_, left, right]) = roots();
        write(&left, "Folder/new", "new");
        write(&right, "folder/old", "old");
        write(&left, "mismatch/child", "file");
        write(&right, "mismatch", "file");
        let plan = build(
            &left,
            &right,
            None,
            Mode::MirrorRight,
            &Cancellation::default(),
        )
        .unwrap();
        for path in [
            "Folder",
            "folder",
            "Folder/new",
            "folder/old",
            "mismatch",
            "mismatch/child",
        ] {
            assert_eq!(item(&plan, path).action, Action::Conflict, "{path}");
        }
        assert_eq!(plan.summary.operations, 0);
    }
    #[cfg(unix)]
    #[test]
    fn unknown_link_subtrees_never_become_delete_candidates() {
        let (_dir, [_, left, right]) = roots();
        fs::create_dir(right.join("target")).unwrap();
        std::os::unix::fs::symlink(&left, right.join("target/link")).unwrap();
        let plan = build(
            &left,
            &right,
            None,
            Mode::MirrorRight,
            &Cancellation::default(),
        )
        .unwrap();
        assert_eq!(item(&plan, "target/link").action, Action::Uncompared);
        assert_eq!(item(&plan, "target").action, Action::Conflict);
        assert_eq!(plan.summary.operations, 0);
        assert_eq!(plan.exit_code(), 2);
    }
}
