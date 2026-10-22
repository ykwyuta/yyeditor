//! Resumable read-only two-way comparisons. No original text or output path is embedded.
use crate::{
    Cancellation, Result,
    files::Snapshot,
    rules::{self, Comparison, Rules},
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
const LIMIT: usize = 64 * 1024;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    path: PathBuf,
    encoding: String,
    blake3: String,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    pub anchor: u64,
    pub head: u64,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Position {
    pub selections: [Selection; 2],
    pub active: usize,
    pub difference: usize,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Session {
    kind: String,
    version: u32,
    inputs: [Input; 2],
    rules: Rules,
    position: Position,
}
pub struct Restored {
    pub inputs: [Snapshot; 2],
    pub comparison: Comparison,
    pub rules: Rules,
    pub position: Position,
}
impl Session {
    pub fn capture(
        inputs: [&Snapshot; 2],
        rules: Rules,
        position: Position,
        cancel: &Cancellation,
    ) -> Result<Self> {
        let comparison = rules::compare(&inputs[0].text, &inputs[1].text, rules, cancel)?;
        validate_position(inputs, position, &comparison)?;
        let mut descriptors = Vec::new();
        for input in inputs {
            input.verify(cancel)?;
            descriptors.push(Input {
                path: input.path.clone(),
                encoding: input.encoding.name().into(),
                blake3: input.fingerprint(cancel)?,
            });
        }
        Ok(Self {
            kind: "yymerge-comparison-session".into(),
            version: 1,
            inputs: descriptors.try_into().map_err(|_| "two inputs required")?,
            rules,
            position,
        })
    }
    pub fn json(&self, cancel: &Cancellation) -> Result<Vec<u8>> {
        cancel.check()?;
        let mut bytes = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        bytes.push(b'\n');
        if bytes.len() > LIMIT {
            return Err("comparison session exceeds 64 KiB limit".into());
        }
        cancel.check()?;
        Ok(bytes)
    }
    pub fn open(path: &Path, cancel: &Cancellation) -> Result<(Restored, Snapshot)> {
        cancel.check()?;
        if std::fs::metadata(path).map_err(|e| e.to_string())?.len() > LIMIT as u64 {
            return Err("comparison session exceeds 64 KiB limit".into());
        }
        let document = Snapshot::open(path, Some("UTF-8"), cancel)?;
        if document.text.len() > LIMIT {
            return Err("comparison session exceeds 64 KiB limit".into());
        }
        let session: Self = serde_json::from_str(&document.text)
            .map_err(|e| format!("invalid comparison session: {e}"))?;
        if session.kind != "yymerge-comparison-session" || session.version != 1 {
            return Err("unsupported comparison session kind/version".into());
        }
        let mut inputs = Vec::new();
        for input in session.inputs {
            cancel.check()?;
            if !input.path.is_absolute() {
                return Err("comparison session paths must be absolute".into());
            }
            let snapshot = Snapshot::open(&input.path, Some(&input.encoding), cancel)?;
            if snapshot.fingerprint(cancel)? != input.blake3 {
                return Err(format!(
                    "comparison session input changed: {}",
                    input.path.display()
                ));
            }
            inputs.push(snapshot);
        }
        let inputs: [Snapshot; 2] = inputs.try_into().map_err(|_| "two inputs required")?;
        let comparison = rules::compare(&inputs[0].text, &inputs[1].text, session.rules, cancel)?;
        validate_position([&inputs[0], &inputs[1]], session.position, &comparison)?;
        for input in &inputs {
            input.verify(cancel)?;
        }
        document.verify(cancel)?;
        Ok((
            Restored {
                inputs,
                comparison,
                rules: session.rules,
                position: session.position,
            },
            document,
        ))
    }
}
fn validate_position(
    inputs: [&Snapshot; 2],
    position: Position,
    comparison: &Comparison,
) -> Result<()> {
    if position.active > 1
        || position.difference >= (comparison.diff.edits.len() + comparison.ignored.len()).max(1)
    {
        return Err("invalid comparison session position".into());
    }
    for (input, selection) in inputs.into_iter().zip(position.selections) {
        for offset in [selection.anchor, selection.head] {
            if offset > input.text.len() as u64 || !input.text.is_char_boundary(offset as usize) {
                return Err("invalid comparison session UTF-8 selection".into());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(dir: &Path) -> [Snapshot; 2] {
        let cancel = Cancellation::default();
        let paths = [dir.join("左.txt"), dir.join("right.txt")];
        std::fs::write(&paths[0], "猫 Alpha\r\n").unwrap();
        std::fs::write(&paths[1], "猫 alpha\n").unwrap();
        paths.map(|path| Snapshot::open(&path, None, &cancel).unwrap())
    }
    #[test]
    fn round_trip_rules_oriented_unicode_selection_and_input_fingerprints() {
        let dir = tempfile::tempdir().unwrap();
        let inputs = fixture(dir.path());
        let cancel = Cancellation::default();
        let rules = Rules {
            ignore_case: true,
            ignore_eol: true,
            ..Default::default()
        };
        let position = Position {
            selections: [
                Selection { anchor: 9, head: 3 },
                Selection { anchor: 4, head: 9 },
            ],
            active: 1,
            difference: 0,
        };
        let session = Session::capture([&inputs[0], &inputs[1]], rules, position, &cancel).unwrap();
        let path = dir.path().join("work.json");
        std::fs::write(&path, session.json(&cancel).unwrap()).unwrap();
        let (restored, _) = Session::open(&path, &cancel).unwrap();
        assert_eq!(restored.rules, rules);
        assert_eq!(restored.position, position);
        assert!(restored.comparison.diff.edits.is_empty());
        assert_eq!(restored.comparison.ignored.len(), 1);
        std::fs::write(&inputs[0].path, "猫 Other\r\n").unwrap();
        assert!(
            Session::open(&path, &cancel)
                .err()
                .unwrap()
                .contains("changed")
        );
    }
    #[test]
    fn malformed_metadata_changed_missing_inputs_and_invalid_positions_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let inputs = fixture(dir.path());
        let cancel = Cancellation::default();
        let session = Session::capture(
            [&inputs[0], &inputs[1]],
            Rules::default(),
            Position::default(),
            &cancel,
        )
        .unwrap();
        let value: serde_json::Value =
            serde_json::from_slice(&session.json(&cancel).unwrap()).unwrap();
        let path = dir.path().join("bad.json");
        for (pointer, bad) in [
            ("/version", serde_json::json!(2)),
            ("/kind", serde_json::json!("other")),
            ("/inputs/0/path", serde_json::json!("relative.txt")),
            ("/inputs/0/blake3", serde_json::json!("bad")),
            ("/position/active", serde_json::json!(2)),
            ("/position/difference", serde_json::json!(20)),
            ("/position/selections/0/anchor", serde_json::json!(1)),
            ("/position/selections/1/head", serde_json::json!(1000)),
        ] {
            let mut bad_value = value.clone();
            *bad_value.pointer_mut(pointer).unwrap() = bad;
            std::fs::write(&path, serde_json::to_vec(&bad_value).unwrap()).unwrap();
            assert!(Session::open(&path, &cancel).is_err(), "{pointer}");
        }
        for bad in ["{\"kind\":\"x\",\"kind\":\"y\"}", "{}"] {
            std::fs::write(&path, bad).unwrap();
            assert!(Session::open(&path, &cancel).is_err());
        }
        let mut extra = value.clone();
        extra["unknown"] = true.into();
        std::fs::write(&path, serde_json::to_vec(&extra).unwrap()).unwrap();
        assert!(Session::open(&path, &cancel).is_err());
        std::fs::write(&path, session.json(&cancel).unwrap()).unwrap();
        std::fs::remove_file(&inputs[0].path).unwrap();
        assert!(Session::open(&path, &cancel).is_err());
        cancel.cancel();
        assert!(session.json(&cancel).is_err());
        assert!(Session::open(&path, &cancel).is_err());
    }
    #[test]
    fn limits_capture_validation_and_raw_encoding_changes_are_not_silently_accepted() {
        let dir = tempfile::tempdir().unwrap();
        let inputs = fixture(dir.path());
        let cancel = Cancellation::default();
        let bad = Position {
            selections: [Selection { anchor: 1, head: 0 }, Selection::default()],
            ..Default::default()
        };
        assert!(
            Session::capture([&inputs[0], &inputs[1]], Rules::default(), bad, &cancel).is_err()
        );
        let session = Session::capture(
            [&inputs[0], &inputs[1]],
            Rules::default(),
            Position::default(),
            &cancel,
        )
        .unwrap();
        let path = dir.path().join("work.json");
        std::fs::write(&path, vec![b' '; LIMIT + 1]).unwrap();
        assert!(Session::open(&path, &cancel).is_err());
        std::fs::write(&path, session.json(&cancel).unwrap()).unwrap();
        std::fs::write(&inputs[0].path, format!("\u{feff}{}", inputs[0].text)).unwrap();
        assert!(Session::open(&path, &cancel).is_err());
    }
}
