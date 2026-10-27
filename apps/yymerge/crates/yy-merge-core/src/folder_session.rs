//! Folder comparison setup; opening always rescans current contents, never reuses a plan.
use crate::{Cancellation, Result, files, folders};
use folders::filter::{Filter, Selection};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Component, Path, PathBuf},
};
const LIMIT: usize = 64 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Session {
    kind: String,
    version: u32,
    pub left: PathBuf,
    pub right: PathBuf,
    pub base: Option<PathBuf>,
    pub recursive: bool,
    pub path_contains: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_glob: Option<String>,
    pub status: Selection,
    pub selected: Option<PathBuf>,
}
fn root(path: &Path) -> Result<PathBuf> {
    let kind = fs::symlink_metadata(path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .file_type();
    if !kind.is_dir() || folders::is_link(path, kind)? {
        return Err("folder session roots must be real directories".into());
    }
    fs::canonicalize(path).map_err(|e| e.to_string())
}
impl Session {
    pub fn capture(
        left: &Path,
        right: &Path,
        base: Option<&Path>,
        recursive: bool,
        filter: &Filter,
        selected: Option<PathBuf>,
        cancel: &Cancellation,
    ) -> Result<Self> {
        cancel.check()?;
        let session = Self {
            kind: "yymerge-folder-session".into(),
            version: 1,
            left: root(left)?,
            right: root(right)?,
            base: base.map(root).transpose()?,
            recursive,
            path_contains: if filter.glob().is_none() {
                filter.query().into()
            } else {
                String::new()
            },
            path_glob: filter.glob().map(str::to_owned),
            status: filter.status(),
            selected,
        };
        session.validate()?;
        cancel.check()?;
        Ok(session)
    }
    fn validate(&self) -> Result<()> {
        if self.kind != "yymerge-folder-session" || self.version != 1 {
            return Err("unsupported folder session kind/version".into());
        }
        for path in [&self.left, &self.right]
            .into_iter()
            .chain(self.base.iter())
        {
            if !path.is_absolute() {
                return Err("folder session root paths must be absolute".into());
            }
        }
        self.filter()?;
        if let Some(path) = &self.selected
            && (path.as_os_str().is_empty()
                || path
                    .components()
                    .any(|c| !matches!(c, Component::Normal(_)))
                || path.to_string_lossy().contains('\0'))
        {
            return Err("folder session selection must be a relative child path".into());
        }
        Ok(())
    }
    pub fn filter(&self) -> Result<Filter> {
        if let Some(pattern) = &self.path_glob {
            if !self.path_contains.is_empty() {
                return Err(
                    "folder session cannot combine literal and wildcard path filters".into(),
                );
            }
            return Filter::wildcard(pattern.clone(), self.status);
        }
        Filter::new(self.path_contains.clone(), self.status)
    }
    pub fn json(&self, cancel: &Cancellation) -> Result<Vec<u8>> {
        cancel.check()?;
        self.validate()?;
        let mut bytes = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        bytes.push(b'\n');
        if bytes.len() > LIMIT {
            return Err("folder session exceeds 64 KiB limit".into());
        }
        cancel.check()?;
        Ok(bytes)
    }
    pub fn open(path: &Path, cancel: &Cancellation) -> Result<Self> {
        cancel.check()?;
        if fs::metadata(path).map_err(|e| e.to_string())?.len() > LIMIT as u64 {
            return Err("folder session exceeds 64 KiB limit".into());
        }
        let document = files::Snapshot::open(path, Some("UTF-8"), cancel)?;
        if document.text.len() > LIMIT {
            return Err("folder session exceeds 64 KiB limit".into());
        }
        let mut session: Self = serde_json::from_str(&document.text)
            .map_err(|e| format!("invalid folder session: {e}"))?;
        session.validate()?;
        session.left = root(&session.left)?;
        session.right = root(&session.right)?;
        session.base = session.base.as_deref().map(root).transpose()?;
        document.verify(cancel)?;
        cancel.check()?;
        Ok(session)
    }
    pub fn save(&self, path: &Path, overwrite: bool, cancel: &Cancellation) -> Result<()> {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let target = fs::canonicalize(parent).map_err(|e| e.to_string())?.join(
            path.file_name()
                .ok_or("session output file name required")?,
        );
        for input in [&self.left, &self.right]
            .into_iter()
            .chain(self.base.iter())
        {
            if target.starts_with(root(input)?) {
                return Err("save the folder session outside every compared root".into());
            }
        }
        files::save(path, &self.json(cancel)?, &[], overwrite, cancel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wildcard_sessions_round_trip_reject_mixed_modes_and_read_legacy() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = setup(dir.path());
        let cancel = Cancellation::default();
        let path = dir.path().join("session.json");
        let original = session.json(&cancel).unwrap();
        assert!(!String::from_utf8_lossy(&original).contains("path_glob"));
        fs::write(&path, original).unwrap();
        assert!(
            Session::open(&path, &cancel)
                .unwrap()
                .filter()
                .unwrap()
                .glob()
                .is_none()
        );
        session.path_contains.clear();
        session.path_glob = Some("*日本語/*.csv".into());
        session.save(&path, true, &cancel).unwrap();
        let loaded = Session::open(&path, &cancel).unwrap();
        assert_eq!(loaded.filter().unwrap().glob(), Some("*日本語/*.csv"));
        session.path_contains = "literal".into();
        assert!(session.json(&cancel).is_err());
        session.path_contains.clear();
        session.path_glob = Some("?".repeat(257));
        assert!(session.json(&cancel).is_err());
        let mut text = String::from_utf8(loaded.json(&cancel).unwrap()).unwrap();
        text = text.replace("\"path_glob\":", "\"path_glob\": \"*\", \"path_glob\":");
        fs::write(&path, text).unwrap();
        assert!(Session::open(&path, &cancel).is_err());
    }
    fn setup(dir: &Path) -> Session {
        for side in ["left", "right", "base"] {
            fs::create_dir(dir.join(side)).unwrap();
        }
        Session::capture(
            &dir.join("left"),
            &dir.join("right"),
            Some(&dir.join("base")),
            false,
            &Filter::new("日本語".into(), Selection::Review).unwrap(),
            Some("日本語/file.txt".into()),
            &Cancellation::default(),
        )
        .unwrap()
    }
    #[test]
    fn round_trip_and_rescan_changed_files_never_restore_old_results() {
        let dir = tempfile::tempdir().unwrap();
        let session = setup(dir.path());
        let cancel = Cancellation::default();
        let path = dir.path().join("work.json");
        session.save(&path, false, &cancel).unwrap();
        let restored = Session::open(&path, &cancel).unwrap();
        assert!(!restored.recursive);
        assert_eq!(restored.base, session.base);
        assert_eq!(restored.status, Selection::Review);
        assert_eq!(restored.selected, session.selected);
        fs::write(session.left.join("new.txt"), "new").unwrap();
        let restored = Session::open(&path, &cancel).unwrap();
        let entries =
            folders::compare(&restored.left, &restored.right, restored.recursive, &cancel).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].status, folders::Status::LeftOnly);
        for target in [
            session.left.join("work.json"),
            session.right.join("work.json"),
            session.base.as_ref().unwrap().join("work.json"),
        ] {
            assert!(session.save(&target, true, &cancel).is_err());
            assert!(!target.exists());
        }
        let bytes = fs::read(&path).unwrap();
        assert!(session.save(&path, false, &cancel).is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
    #[test]
    fn invalid_metadata_missing_roots_limits_and_cancel_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let session = setup(dir.path());
        let cancel = Cancellation::default();
        let path = dir.path().join("work.json");
        let value: serde_json::Value =
            serde_json::from_slice(&session.json(&cancel).unwrap()).unwrap();
        for (key, bad) in [
            ("kind", serde_json::json!("unknown")),
            ("version", serde_json::json!(2)),
            ("left", serde_json::json!("relative")),
            ("selected", serde_json::json!("../escape")),
            ("status", serde_json::json!("bad")),
            ("path_contains", serde_json::json!("x".repeat(2049))),
        ] {
            let mut v = value.clone();
            v[key] = bad;
            fs::write(&path, serde_json::to_vec(&v).unwrap()).unwrap();
            assert!(Session::open(&path, &cancel).is_err(), "{key}");
        }
        for bad in ["{}", "{\"kind\":\"x\",\"kind\":\"x\"}"] {
            fs::write(&path, bad).unwrap();
            assert!(Session::open(&path, &cancel).is_err());
        }
        let mut extra = value.clone();
        extra["unexpected"] = true.into();
        fs::write(&path, serde_json::to_vec(&extra).unwrap()).unwrap();
        assert!(Session::open(&path, &cancel).is_err());
        fs::write(&path, vec![b' '; LIMIT + 1]).unwrap();
        assert!(Session::open(&path, &cancel).is_err());
        session.save(&path, true, &cancel).unwrap();
        fs::remove_dir(&session.left).unwrap();
        assert!(Session::open(&path, &cancel).is_err());
        cancel.cancel();
        assert!(session.json(&cancel).is_err());
        assert!(Session::open(&path, &cancel).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn linked_root_is_rejected_and_link_children_remain_uncompared() {
        let dir = tempfile::tempdir().unwrap();
        let session = setup(dir.path());
        let cancel = Cancellation::default();
        let link = dir.path().join("linked");
        std::os::unix::fs::symlink(&session.left, &link).unwrap();
        assert!(
            Session::capture(
                &link,
                &session.right,
                None,
                true,
                &Filter::default(),
                None,
                &cancel
            )
            .is_err()
        );
        std::os::unix::fs::symlink("missing", session.left.join("child")).unwrap();
        assert_eq!(
            folders::compare(&session.left, &session.right, true, &cancel).unwrap()[0].status,
            folders::Status::SkippedLink
        );
    }
}
