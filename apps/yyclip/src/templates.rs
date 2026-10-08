//! 定型文を yyclip 専用のテキストファイルとして保存する。

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::history::MAX_BYTES;

pub(crate) const LIMIT: usize = 20;

pub(crate) fn default_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|p| PathBuf::from(p).join("yyclip").join("templates"))
}

pub(crate) struct Store {
    dir: PathBuf,
}

impl Store {
    pub(crate) fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub(crate) fn entries(&self) -> Vec<(PathBuf, String)> {
        (1..=LIMIT)
            .filter_map(|slot| {
                let path = self.path(slot);
                let size = path.metadata().ok()?.len();
                if size == 0 || size > MAX_BYTES as u64 {
                    return None;
                }
                let text = fs::read_to_string(&path).ok()?;
                valid(&text).then_some((path, text))
            })
            .collect()
    }

    pub(crate) fn add(&self, text: &str) -> io::Result<PathBuf> {
        if !valid(text) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "定型文のサイズが対象外",
            ));
        }
        fs::create_dir_all(&self.dir)?;
        if self.entries().iter().any(|(_, existing)| existing == text) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "同じ定型文が登録済み",
            ));
        }
        for slot in 1..=LIMIT {
            let path = self.path(slot);
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    if let Err(error) = file.write_all(text.as_bytes()) {
                        drop(file);
                        let _ = fs::remove_file(&path);
                        return Err(error);
                    }
                    return Ok(path);
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::StorageFull,
            "定型文は 20 件までです",
        ))
    }

    pub(crate) fn update(&self, path: &Path, text: &str) -> io::Result<()> {
        if !(1..=LIMIT).any(|slot| self.path(slot) == path) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "定型文の場所が不正",
            ));
        }
        if !valid(text) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "定型文のサイズが対象外",
            ));
        }
        if self
            .entries()
            .iter()
            .any(|(entry_path, existing)| entry_path != path && existing == text)
        {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "同じ定型文が登録済み",
            ));
        }
        // 存在しないスロットに更新から新規登録させない。
        OpenOptions::new().write(true).open(path)?;
        fs::write(path, text)
    }

    pub(crate) fn remove(&self, path: &Path) -> io::Result<()> {
        if !(1..=LIMIT).any(|slot| self.path(slot) == path) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "定型文の場所が不正",
            ));
        }
        fs::remove_file(path)
    }

    fn path(&self, slot: usize) -> PathBuf {
        self.dir.join(format!("{slot:02}.txt"))
    }
}

fn valid(text: &str) -> bool {
    !text.is_empty() && text.len() <= MAX_BYTES
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn persists_and_limits_to_twenty() {
        let dir = std::env::temp_dir().join(format!(
            "yyclip-templates-{}-{}",
            std::process::id(),
            NEXT_TEST.fetch_add(1, Ordering::Relaxed)
        ));
        let store = Store::new(dir.clone());
        for slot in 1..=LIMIT {
            store.add(&format!("定型文 {slot}")).unwrap();
        }
        assert_eq!(Store::new(dir.clone()).entries().len(), LIMIT);
        assert_eq!(
            store.add("追加").unwrap_err().kind(),
            io::ErrorKind::StorageFull
        );
        assert_eq!(
            store.add("定型文 1").unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        let first = store.entries()[0].0.clone();
        store.remove(&first).unwrap();
        assert_eq!(store.add("追加").unwrap(), first);
        store.update(&first, "任意の\n複数行テキスト").unwrap();
        assert_eq!(
            fs::read_to_string(&first).unwrap(),
            "任意の\n複数行テキスト"
        );
        assert_eq!(
            store.update(&first, "定型文 2").unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(store.entries().len(), LIMIT);
        assert_eq!(
            store.add("").unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            store.add(&"x".repeat(MAX_BYTES + 1)).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        fs::remove_dir_all(dir).unwrap();
    }
}
