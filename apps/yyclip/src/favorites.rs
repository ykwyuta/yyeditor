//! yyclip のお気に入りファイル・フォルダを保存する。

use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

pub(crate) const LIMIT: usize = 20;
const MAX_PATH_UNITS: usize = 32_767;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Kind {
    File,
    Folder,
}

impl Kind {
    fn marker(self) -> u8 {
        match self {
            Self::File => 1,
            Self::Folder => 2,
        }
    }

    fn from_marker(marker: u8) -> Option<Self> {
        match marker {
            1 => Some(Self::File),
            2 => Some(Self::Folder),
            _ => None,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::File => "ファイル",
            Self::Folder => "フォルダ",
        }
    }

    fn matches(self, path: &Path) -> bool {
        match self {
            Self::File => path.is_file(),
            Self::Folder => path.is_dir(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Favorite {
    pub(crate) kind: Kind,
    pub(crate) target: PathBuf,
}

impl Favorite {
    pub(crate) fn label(&self) -> String {
        let name = self
            .target
            .file_name()
            .unwrap_or_else(|| self.target.as_os_str())
            .to_string_lossy();
        format!(
            "{}: {} — {}",
            self.kind.label(),
            name,
            self.target.display()
        )
    }
}

pub(crate) fn default_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|p| PathBuf::from(p).join("yyclip").join("favorites"))
}

pub(crate) struct Store {
    dir: PathBuf,
}

impl Store {
    pub(crate) fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub(crate) fn entries(&self) -> Vec<(PathBuf, Favorite)> {
        (1..=LIMIT)
            .filter_map(|slot| {
                let path = self.path(slot);
                let bytes = fs::read(&path).ok()?;
                decode(&bytes).map(|favorite| (path, favorite))
            })
            .collect()
    }

    pub(crate) fn add(&self, favorite: &Favorite) -> io::Result<PathBuf> {
        let bytes = encode(favorite)?;
        self.check_duplicate(favorite, None)?;
        fs::create_dir_all(&self.dir)?;
        for slot in 1..=LIMIT {
            let path = self.path(slot);
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    if let Err(error) = file.write_all(&bytes) {
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
            "お気に入りはファイル・フォルダ合わせて 20 件までです",
        ))
    }

    pub(crate) fn update(&self, slot_path: &Path, favorite: &Favorite) -> io::Result<()> {
        self.check_slot(slot_path)?;
        let bytes = encode(favorite)?;
        self.check_duplicate(favorite, Some(slot_path))?;
        OpenOptions::new().write(true).open(slot_path)?;
        fs::write(slot_path, bytes)
    }

    pub(crate) fn remove(&self, slot_path: &Path) -> io::Result<()> {
        self.check_slot(slot_path)?;
        fs::remove_file(slot_path)
    }

    fn check_slot(&self, path: &Path) -> io::Result<()> {
        if (1..=LIMIT).any(|slot| self.path(slot) == path) {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "お気に入りの保存先が不正です",
            ))
        }
    }

    fn check_duplicate(&self, favorite: &Favorite, except: Option<&Path>) -> io::Result<()> {
        if self.entries().iter().any(|(slot, entry)| {
            Some(slot.as_path()) != except
                && entry
                    .target
                    .to_string_lossy()
                    .eq_ignore_ascii_case(&favorite.target.to_string_lossy())
        }) {
            Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "同じファイルまたはフォルダは登録済みです",
            ))
        } else {
            Ok(())
        }
    }

    fn path(&self, slot: usize) -> PathBuf {
        self.dir.join(format!("{slot:02}.bin"))
    }
}

fn encode(favorite: &Favorite) -> io::Result<Vec<u8>> {
    let wide: Vec<u16> = favorite.target.as_os_str().encode_wide().collect();
    if wide.is_empty()
        || wide.len() > MAX_PATH_UNITS
        || wide.contains(&0)
        || !favorite.kind.matches(&favorite.target)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "存在するファイルまたはフォルダを選択してください",
        ));
    }
    let mut bytes = Vec::with_capacity(1 + wide.len() * 2);
    bytes.push(favorite.kind.marker());
    for unit in wide {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    Ok(bytes)
}

fn decode(bytes: &[u8]) -> Option<Favorite> {
    if bytes.len() < 3 || bytes.len() > 1 + MAX_PATH_UNITS * 2 || bytes.len() % 2 == 0 {
        return None;
    }
    let kind = Kind::from_marker(bytes[0])?;
    let wide: Vec<u16> = bytes[1..]
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    if wide.contains(&0) {
        return None;
    }
    Some(Favorite {
        kind,
        target: PathBuf::from(OsString::from_wide(&wide)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn persists_files_and_folders_with_shared_twenty_slot_limit() {
        let dir = std::env::temp_dir().join(format!(
            "yyclip-favorites-{}-{}",
            std::process::id(),
            NEXT_TEST.fetch_add(1, Ordering::Relaxed)
        ));
        let targets = dir.join("targets");
        fs::create_dir_all(&targets).unwrap();
        let folder = Favorite {
            kind: Kind::Folder,
            target: targets.clone(),
        };
        let store = Store::new(dir.join("saved"));
        let first = store.add(&folder).unwrap();
        assert_eq!(Store::new(dir.join("saved")).entries()[0].1, folder);
        assert_eq!(
            store.add(&folder).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        for n in 1..LIMIT {
            let path = targets.join(format!("file-{n}.txt"));
            fs::write(&path, "x").unwrap();
            store
                .add(&Favorite {
                    kind: Kind::File,
                    target: path,
                })
                .unwrap();
        }
        assert_eq!(store.entries().len(), LIMIT);
        let extra = targets.join("extra.txt");
        fs::write(&extra, "x").unwrap();
        let replacement = Favorite {
            kind: Kind::File,
            target: extra,
        };
        assert_eq!(
            store.add(&replacement).unwrap_err().kind(),
            io::ErrorKind::StorageFull
        );
        store.update(&first, &replacement).unwrap();
        assert_eq!(store.entries()[0].1, replacement);
        store.remove(&first).unwrap();
        assert_eq!(store.entries().len(), LIMIT - 1);
        fs::remove_dir_all(dir).unwrap();
    }
}
