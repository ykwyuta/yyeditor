//! Explicit work saves. Restore remerges verified inputs before replaying decisions.
use crate::{Cancellation, Choice, MAX_BYTES, Merge, Part, Result, files::Snapshot};
use serde::{Deserialize, Serialize};
use std::{
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    path: PathBuf,
    encoding: String,
    blake3: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Decision {
    id: usize,
    fingerprint: String,
    choice: Option<Choice>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Session {
    kind: String,
    version: u32,
    /// Fixed role order: BASE, LEFT, RIGHT. No result destination is persisted.
    inputs: [Input; 3],
    decisions: Vec<Decision>,
    pub result: String,
    pub manual: bool,
    pub selected: usize,
}
pub struct Restored {
    pub inputs: [Snapshot; 3],
    pub model: Merge,
    pub result: String,
    pub manual: bool,
    pub selected: usize,
}
fn conflict_fingerprint(conflict: &crate::Conflict, cancel: &Cancellation) -> Result<String> {
    let mut hash = blake3::Hasher::new();
    hash.update(b"yymerge session conflict v1\0");
    for n in [
        conflict.base_lines.start,
        conflict.base_lines.end,
        conflict.left_lines.start,
        conflict.left_lines.end,
        conflict.right_lines.start,
        conflict.right_lines.end,
    ] {
        hash.update(&(n as u64).to_le_bytes());
    }
    for text in [&conflict.base, &conflict.left, &conflict.right] {
        hash.update(&(text.len() as u64).to_le_bytes());
        for block in text.as_bytes().chunks(64 * 1024) {
            cancel.check()?;
            hash.update(block);
        }
    }
    cancel.check()?;
    Ok(hash.finalize().to_hex().to_string())
}
impl Session {
    pub fn capture(
        inputs: [&Snapshot; 3],
        model: &Merge,
        result: String,
        manual: bool,
        selected: usize,
        cancel: &Cancellation,
    ) -> Result<Self> {
        let mut descriptors = Vec::new();
        for input in inputs {
            input.verify(cancel)?;
            descriptors.push(Input {
                path: input.path.clone(),
                encoding: input.encoding.name().into(),
                blake3: input.fingerprint(cancel)?,
            });
        }
        validate_encodings(inputs)?;
        let mut decisions = Vec::new();
        for part in &model.parts {
            if let Part::Conflict(conflict) = part {
                decisions.push(Decision {
                    id: conflict.id,
                    fingerprint: conflict_fingerprint(conflict, cancel)?,
                    choice: conflict.resolution.clone(),
                });
            }
        }
        let session = Self {
            kind: "yymerge-merge-session".into(),
            version: 1,
            inputs: descriptors
                .try_into()
                .map_err(|_| "three session inputs required")?,
            decisions,
            result,
            manual,
            selected,
        };
        session.validate_result(model, cancel)?;
        Ok(session)
    }
    fn validate_result(&self, model: &Merge, cancel: &Cancellation) -> Result<()> {
        cancel.check()?;
        if self.result.len() as u64 > MAX_BYTES || self.result.contains('\0') {
            return Err("invalid session result: NUL or 32 MiB limit".into());
        }
        if self.selected >= self.decisions.len().max(1) {
            return Err("invalid session conflict selection".into());
        }
        if self
            .decisions
            .iter()
            .any(|d| matches!(&d.choice, Some(Choice::Manual(t)) if t.contains('\0')))
        {
            return Err("NUL in session conflict text".into());
        }
        if !self.manual && model.render(true)? != self.result {
            return Err("session result does not match conflict decisions".into());
        }
        cancel.check()
    }
    pub fn json(&self, cancel: &Cancellation) -> Result<Vec<u8>> {
        self.json_limited(cancel, MAX_BYTES as usize)
    }
    fn json_limited(&self, cancel: &Cancellation, limit: usize) -> Result<Vec<u8>> {
        struct Output<'a> {
            bytes: Vec<u8>,
            cancel: &'a Cancellation,
            limit: usize,
        }
        impl Write for Output<'_> {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.cancel.check().map_err(std::io::Error::other)?;
                if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
                    return Err(std::io::Error::other(
                        "session exceeds 32 MiB serialized limit",
                    ));
                }
                self.bytes.extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut out = Output {
            bytes: Vec::new(),
            cancel,
            limit,
        };
        serde_json::to_writer_pretty(&mut out, self).map_err(|e| {
            if cancel.check().is_err() {
                "cancelled".into()
            } else {
                e.to_string()
            }
        })?;
        out.write_all(b"\n").map_err(|e| e.to_string())?;
        Ok(out.bytes)
    }
    pub fn open(path: &Path, cancel: &Cancellation) -> Result<(Restored, Snapshot)> {
        let document = Snapshot::open(path, Some("UTF-8"), cancel)?;
        let session: Self = serde_json::from_str(&document.text)
            .map_err(|e| format!("invalid merge session: {e}"))?;
        if session.version != 1 || session.kind != "yymerge-merge-session" {
            return Err("unsupported merge session kind/version".into());
        }
        let mut inputs = Vec::new();
        for input in &session.inputs {
            cancel.check()?;
            if !input.path.is_absolute() {
                return Err("session input paths must be absolute".into());
            }
            let snapshot = Snapshot::open(&input.path, Some(&input.encoding), cancel)?;
            if snapshot.fingerprint(cancel)? != input.blake3 {
                return Err(format!("session input changed: {}", input.path.display()));
            }
            inputs.push(snapshot);
        }
        let inputs: [Snapshot; 3] = inputs
            .try_into()
            .map_err(|_| "three session inputs required")?;
        validate_encodings([&inputs[0], &inputs[1], &inputs[2]])?;
        let mut model = crate::merge(&inputs[0].text, &inputs[1].text, &inputs[2].text, cancel)?;
        let mut decisions = session.decisions.iter();
        for part in &mut model.parts {
            if let Part::Conflict(conflict) = part {
                cancel.check()?;
                let decision = decisions
                    .next()
                    .ok_or("missing session conflict decision")?;
                if decision.id != conflict.id
                    || decision.fingerprint != conflict_fingerprint(conflict, cancel)?
                {
                    return Err("session conflict identity/order mismatch".into());
                }
                if let Some(Choice::Manual(text)) = &decision.choice
                    && text.contains('\0')
                {
                    return Err("NUL in session conflict text".into());
                }
                conflict.resolution = decision.choice.clone();
            }
        }
        if decisions.next().is_some() {
            return Err("unknown session conflict decision".into());
        }
        session.validate_result(&model, cancel)?;
        for input in &inputs {
            input.verify(cancel)?;
        }
        document.verify(cancel)?;
        Ok((
            Restored {
                inputs,
                model,
                result: session.result,
                manual: session.manual,
                selected: session.selected,
            },
            document,
        ))
    }
}
fn validate_encodings(inputs: [&Snapshot; 3]) -> Result<()> {
    if inputs
        .iter()
        .any(|s| s.encoding != inputs[0].encoding || s.bom != inputs[0].bom)
    {
        return Err("merge session inputs require matching encoding and BOM".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, [Snapshot; 3], Merge) {
        let dir = tempfile::tempdir().unwrap();
        let c = Cancellation::default();
        let inputs = ["base", "left", "right"].map(|name| {
            let path = dir.path().join(name);
            let text = match name {
                "base" => "a\nkeep\nb\n",
                "left" => "A\nkeep\nB\n",
                _ => "X\nkeep\nY\n",
            };
            std::fs::write(&path, text).unwrap();
            Snapshot::open(&path, None, &c).unwrap()
        });
        let m = crate::merge(&inputs[0].text, &inputs[1].text, &inputs[2].text, &c).unwrap();
        (dir, inputs, m)
    }
    #[test]
    fn decisions_unresolved_and_manual_result_survive_verified_remerge() {
        let (dir, inputs, mut model) = fixture();
        let c = Cancellation::default();
        let id = model
            .parts
            .iter()
            .find_map(|p| match p {
                Part::Conflict(c) => Some(c.id),
                _ => None,
            })
            .unwrap();
        model.resolve(id, Some(Choice::Both)).unwrap();
        let path = dir.path().join("work.yymerge-session.json");
        for manual in [false, true] {
            let text = if manual {
                "手動の未完了 RESULT\r\n".into()
            } else {
                model.render(true).unwrap()
            };
            let session = Session::capture(
                [&inputs[0], &inputs[1], &inputs[2]],
                &model,
                text.clone(),
                manual,
                1,
                &c,
            )
            .unwrap();
            std::fs::write(&path, session.json(&c).unwrap()).unwrap();
            let (restored, _) = Session::open(&path, &c).unwrap();
            assert_eq!(restored.model.unresolved(), 1);
            assert_eq!(
                restored.model.render(true).unwrap(),
                model.render(true).unwrap()
            );
            assert_eq!(restored.result, text);
            assert_eq!(restored.manual, manual);
            assert_eq!(restored.selected, 1);
        }
        std::fs::write(&inputs[1].path, "changed").unwrap();
        assert!(Session::open(&path, &c).err().unwrap().contains("changed"));
    }
    #[test]
    fn malformed_changed_and_mismatched_sessions_are_rejected() {
        let (dir, inputs, model) = fixture();
        let c = Cancellation::default();
        let state = Session::capture(
            [&inputs[0], &inputs[1], &inputs[2]],
            &model,
            model.render(true).unwrap(),
            false,
            0,
            &c,
        )
        .unwrap();
        let original: serde_json::Value = serde_json::from_slice(&state.json(&c).unwrap()).unwrap();
        let path = dir.path().join("session.json");
        for field in [
            "version",
            "kind",
            "result",
            "selected",
            "decisions",
            "unknown",
            "relative",
            "fingerprint",
            "conflict-fingerprint",
            "missing-decision",
            "extra-decision",
            "nul",
            "choice",
            "missing-result",
        ] {
            let mut value = original.clone();
            match field {
                "version" => value["version"] = 99.into(),
                "kind" => value["kind"] = "other".into(),
                "result" => value["result"] = "lost decisions".into(),
                "selected" => value["selected"] = 999.into(),
                "decisions" => value["decisions"][0]["id"] = 999.into(),
                "unknown" => value["execute"] = "anything".into(),
                "relative" => value["inputs"][0]["path"] = "base".into(),
                "conflict-fingerprint" => {
                    value["decisions"][0]["fingerprint"] = "0".repeat(64).into()
                }
                "missing-decision" => {
                    value["decisions"].as_array_mut().unwrap().pop();
                }
                "extra-decision" => {
                    let duplicate = value["decisions"][0].clone();
                    value["decisions"].as_array_mut().unwrap().push(duplicate);
                }
                "nul" => {
                    value["manual"] = true.into();
                    value["result"] = "bad\0text".into();
                }
                "choice" => value["decisions"][0]["choice"] = "Unknown".into(),
                "missing-result" => {
                    value.as_object_mut().unwrap().remove("result");
                }
                _ => value["inputs"][0]["blake3"] = "0".repeat(64).into(),
            }
            std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
            assert!(Session::open(&path, &c).is_err(), "{field}");
        }
        assert!(state.json_limited(&c, 100).is_err());
        c.cancel();
        assert_eq!(state.json(&c).unwrap_err(), "cancelled");
    }
    #[test]
    fn every_choice_and_confirmed_manual_work_restore_without_output_writes() {
        let (dir, inputs, initial) = fixture();
        let c = Cancellation::default();
        let path = dir.path().join("work.json");
        let ids: Vec<_> = initial
            .parts
            .iter()
            .filter_map(|p| match p {
                Part::Conflict(c) => Some(c.id),
                _ => None,
            })
            .collect();
        for choice in [
            None,
            Some(Choice::Left),
            Some(Choice::Right),
            Some(Choice::Base),
            Some(Choice::Both),
            Some(Choice::Manual("選択した本文\r\n".into())),
        ] {
            let mut model = initial.clone();
            for id in &ids {
                model.resolve(*id, choice.clone()).unwrap();
            }
            for manual in [false, true] {
                let result = if manual {
                    "手動で確定したRESULT\n".into()
                } else {
                    model.render(true).unwrap()
                };
                let state = Session::capture(
                    [&inputs[0], &inputs[1], &inputs[2]],
                    &model,
                    result.clone(),
                    manual,
                    0,
                    &c,
                )
                .unwrap();
                std::fs::write(&path, state.json(&c).unwrap()).unwrap();
                let (restored, _) = Session::open(&path, &c).unwrap();
                assert_eq!(restored.result, result);
                assert_eq!(
                    restored.model.unresolved(),
                    if choice.is_none() { ids.len() } else { 0 }
                );
                assert_eq!(
                    restored.model.render(true).unwrap(),
                    model.render(true).unwrap()
                );
            }
        }
        for input in &inputs {
            input.verify(&c).unwrap();
        }
    }
}
