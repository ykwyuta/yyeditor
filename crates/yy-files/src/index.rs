//! 目録とハッシュのキャッシュ（索引。18 章 3）。
//!
//! 大きさ・更新日時・ファイル ID が前と同じファイルは、前に求めたハッシュをそのまま使う。ファイルは
//! postcard で詰めて miniz で圧縮し、一時ファイルに書いてから置き換える。

use std::collections::HashMap;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::fs::Meta;
use crate::hash::Hash;

/// ファイルの形式の印。
const MAGIC: &[u8; 8] = b"YYFMIDX1";

/// 1 ファイルの覚え書き。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cached {
    pub size: u64,
    pub mtime: i64,
    pub file_id: Option<u128>,
    /// 大きさと先頭・末尾のハッシュ
    pub edges: Option<Hash>,
    /// 全体のハッシュ
    pub full: Option<Hash>,
}

/// 1 つのルートの索引。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Index {
    pub root: String,
    /// 相対パス → 覚え書き
    pub files: HashMap<String, Cached>,
}

impl Index {
    pub fn new(root: &Path) -> Index {
        Index {
            root: root.to_string_lossy().into_owned(),
            files: HashMap::new(),
        }
    }

    /// 前と同じファイルなら、その覚え書き。
    pub fn lookup(&self, rel: &str, m: &Meta) -> Option<&Cached> {
        self.files
            .get(rel)
            .filter(|c| c.size == m.size && c.mtime == m.mtime && c.file_id == m.file_id)
    }

    /// 覚え書きを書き換える（ファイルが変わっていればハッシュを捨てる）。
    pub fn entry(&mut self, rel: &str, m: &Meta) -> &mut Cached {
        let c = self.files.entry(rel.to_owned()).or_default();
        if c.size != m.size || c.mtime != m.mtime || c.file_id != m.file_id {
            *c = Cached {
                size: m.size,
                mtime: m.mtime,
                file_id: m.file_id,
                edges: None,
                full: None,
            };
        }
        c
    }

    /// 目録にないファイルの覚え書きを捨てる。
    pub fn retain(&mut self, keep: &dyn Fn(&str) -> bool) {
        self.files.retain(|k, _| keep(k));
    }

    pub fn load(path: &Path) -> io::Result<Index> {
        let raw = std::fs::read(path)?;
        let body = raw
            .strip_prefix(MAGIC.as_slice())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "索引の形式が違います"))?;
        let plain = miniz_oxide::inflate::decompress_to_vec(body)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{e:?}")))?;
        postcard::from_bytes(&plain).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    /// 保存する（一時ファイルに書いてから置き換える）。
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let plain = postcard::to_allocvec(self).map_err(io::Error::other)?;
        let packed = miniz_oxide::deflate::compress_to_vec(&plain, 3);
        write_atomic(path, &[MAGIC.as_slice(), &packed].concat())
    }
}

/// 索引のファイルの置き場所（`dir` の下の、ルートの名前から作った名前）。
pub fn path_for(dir: &Path, root: &Path) -> PathBuf {
    let key = crate::rel_key(&root.to_string_lossy());
    let h = crate::hash::bytes(key.as_bytes());
    let name: String = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, '-' | '_'))
        .take(24)
        .collect();
    dir.join(format!("{name}-{}.idx", &crate::hash::hex(&h)[..16]))
}

/// 一時ファイルに書いてディスクに書き出し、置き換える（前か後のどちらかが必ず残る）。
pub fn write_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    let r = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if r.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_and_reuses() {
        let d = tempfile::tempdir().unwrap();
        let mut ix = Index::new(Path::new(r"\\nas\share\案件"));
        let m = Meta {
            size: 10,
            mtime: 5,
            ..Meta::default()
        };
        ix.entry("a.txt", &m).full = Some([7; 32]);
        let p = path_for(d.path(), Path::new(r"\\nas\share\案件"));
        assert!(p.file_name().unwrap().to_string_lossy().ends_with(".idx"));
        ix.save(&p).unwrap();
        let back = Index::load(&p).unwrap();
        assert_eq!(back, ix);
        assert_eq!(back.lookup("a.txt", &m).unwrap().full, Some([7; 32]));
        // 変わったファイルは使わない
        let m2 = Meta { mtime: 6, ..m };
        assert!(back.lookup("a.txt", &m2).is_none());
        let mut ix = back;
        assert_eq!(ix.entry("a.txt", &m2).full, None);
        ix.retain(&|r| r != "a.txt");
        assert!(ix.files.is_empty());
        std::fs::write(&p, b"broken").unwrap();
        assert!(Index::load(&p).is_err());
    }
}
