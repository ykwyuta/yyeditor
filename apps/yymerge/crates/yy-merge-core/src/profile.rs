//! Portable, versioned display rules. Profiles contain no paths or executable actions.
use crate::{Cancellation, Result, files::Snapshot, rules::Rules};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    version: u32,
    pub rules: Rules,
}
impl Profile {
    pub fn new(rules: Rules) -> Self {
        Self { version: 1, rules }
    }
    pub fn json(&self) -> Result<String> {
        serde_json::to_string_pretty(self)
            .map(|s| s + "\n")
            .map_err(|e| e.to_string())
    }
    pub fn open(path: &Path, cancel: &Cancellation) -> Result<(Self, Snapshot)> {
        cancel.check()?;
        if std::fs::metadata(path).map_err(|e| e.to_string())?.len() > 65536 {
            return Err("comparison profile exceeds 64 KiB".into());
        }
        let snapshot = Snapshot::open(path, Some("UTF-8"), cancel)?;
        if snapshot.text.len() > 65536 {
            return Err("comparison profile exceeds 64 KiB".into());
        }
        let profile: Self = serde_json::from_str(&snapshot.text)
            .map_err(|e| format!("invalid comparison profile: {e}"))?;
        if profile.version != 1 {
            return Err(format!(
                "unsupported comparison profile version: {}",
                profile.version
            ));
        }
        snapshot.verify(cancel)?;
        Ok((profile, snapshot))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn portable_round_trip_and_strict_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("規則.json");
        let rules = Rules {
            ignore_case: true,
            ignore_eol: true,
            ..Default::default()
        };
        let text = Profile::new(rules).json().unwrap();
        std::fs::write(&path, &text).unwrap();
        let cancel = Cancellation::default();
        let (loaded, snapshot) = Profile::open(&path, &cancel).unwrap();
        assert_eq!(loaded.rules, rules);
        assert_eq!(&*snapshot.text, text);
        for invalid in [
            text.replace("\"version\": 1", "\"version\": 2"),
            text.replace("\"version\": 1", "\"version\": 1, \"action\": \"run\""),
            text.replace(
                "\"ignore_case\": true",
                "\"ignore_case\": true, \"unknown\": false",
            ),
            text.replace("\"ignore_case\": true,", ""),
            text.replace("\"version\": 1", "\"version\": 1, \"version\": 1"),
            "{}".into(),
            " ".repeat(65537),
        ] {
            std::fs::write(&path, invalid).unwrap();
            assert!(Profile::open(&path, &cancel).is_err());
        }
        cancel.cancel();
        assert_eq!(Profile::open(&path, &cancel).err().unwrap(), "cancelled");
    }
}
