//! 定型文を yyclip 専用のテキストファイルとして保存する。
//!
//! 本文は `NN.txt`、それが何を意味するかのメモ（1 行。なくてもよい）は隣の `NN.memo.txt` に置く。

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::history::MAX_BYTES;

pub(crate) const LIMIT: usize = 20;
/// メモの長さの上限（文字）。
pub(crate) const MEMO_LIMIT: usize = 200;

/// 1 件の定型文。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Template {
    pub(crate) path: PathBuf,
    pub(crate) text: String,
    /// 何に使う定型文かのメモ（空なら付けていない）
    pub(crate) memo: String,
}

impl Template {
    /// 一覧に出す名前（メモがあれば「【メモ】本文の先頭」）。
    pub(crate) fn label(&self) -> String {
        let body = crate::history::label(&self.text);
        if self.memo.is_empty() {
            body
        } else {
            format!("【{}】{body}", self.memo)
        }
    }
}

/// メモを 1 行にそろえる（改行・制御文字は空白に、前後の空白は除き、長さを抑える）。
pub(crate) fn clean_memo(memo: &str) -> String {
    let one: String = memo
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    one.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MEMO_LIMIT)
        .collect()
}

/// 本文のファイルに対するメモのファイル（`NN.txt` → `NN.memo.txt`）。
fn memo_path(path: &Path) -> PathBuf {
    path.with_extension("memo.txt")
}

fn read_memo(path: &Path) -> String {
    fs::read_to_string(memo_path(path))
        .map(|m| clean_memo(&m))
        .unwrap_or_default()
}

/// メモを書く（空ならメモのファイルを消す）。
fn write_memo(path: &Path, memo: &str) -> io::Result<()> {
    let memo = clean_memo(memo);
    let p = memo_path(path);
    if memo.is_empty() {
        match fs::remove_file(&p) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    } else {
        fs::write(p, memo)
    }
}

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

    pub(crate) fn entries(&self) -> Vec<Template> {
        (1..=LIMIT)
            .filter_map(|slot| {
                let path = self.path(slot);
                let size = path.metadata().ok()?.len();
                if size == 0 || size > MAX_BYTES as u64 {
                    return None;
                }
                let text = fs::read_to_string(&path).ok()?;
                let memo = read_memo(&path);
                valid(&text).then_some(Template { path, text, memo })
            })
            .collect()
    }

    pub(crate) fn add(&self, text: &str, memo: &str) -> io::Result<PathBuf> {
        if !valid(text) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "定型文のサイズが対象外",
            ));
        }
        fs::create_dir_all(&self.dir)?;
        if self.entries().iter().any(|t| t.text == text) {
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
                    // 前に同じ番号で消し残したメモがあっても、ここで書き直す
                    write_memo(&path, memo)?;
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

    pub(crate) fn update(&self, path: &Path, text: &str, memo: &str) -> io::Result<()> {
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
            .any(|t| t.path != path && t.text == text)
        {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "同じ定型文が登録済み",
            ));
        }
        // 存在しないスロットに更新から新規登録させない。
        OpenOptions::new().write(true).open(path)?;
        fs::write(path, text)?;
        write_memo(path, memo)
    }

    pub(crate) fn remove(&self, path: &Path) -> io::Result<()> {
        if !(1..=LIMIT).any(|slot| self.path(slot) == path) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "定型文の場所が不正",
            ));
        }
        fs::remove_file(path)?;
        write_memo(path, "")
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
            store.add(&format!("定型文 {slot}"), "").unwrap();
        }
        assert_eq!(Store::new(dir.clone()).entries().len(), LIMIT);
        assert_eq!(
            store.add("追加", "").unwrap_err().kind(),
            io::ErrorKind::StorageFull
        );
        assert_eq!(
            store.add("定型文 1", "").unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        let first = store.entries()[0].path.clone();
        store.remove(&first).unwrap();
        assert_eq!(store.add("追加", "").unwrap(), first);
        store.update(&first, "任意の\n複数行テキスト", "").unwrap();
        assert_eq!(
            fs::read_to_string(&first).unwrap(),
            "任意の\n複数行テキスト"
        );
        assert_eq!(
            store.update(&first, "定型文 2", "").unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(store.entries().len(), LIMIT);
        assert_eq!(
            store.add("", "").unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            store
                .add(&"x".repeat(MAX_BYTES + 1), "")
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn keeps_a_memo_per_template() {
        let dir = std::env::temp_dir().join(format!(
            "yyclip-templates-{}-{}",
            std::process::id(),
            NEXT_TEST.fetch_add(1, Ordering::Relaxed)
        ));
        let store = Store::new(dir.clone());
        let a = store
            .add("〒100-0001 東京都千代田区…", "  会社の住所\n（請求書用）  ")
            .unwrap();
        let b = store.add("いつもお世話になっております。", "").unwrap();
        let e = store.entries();
        assert_eq!(e[0].memo, "会社の住所 （請求書用）");
        assert_eq!(
            e[0].label(),
            "【会社の住所 （請求書用）】〒100-0001 東京都千代田区…"
        );
        assert_eq!(e[1].memo, "");
        assert_eq!(e[1].label(), "いつもお世話になっております。");
        assert!(memo_path(&a).exists() && !memo_path(&b).exists());
        // 編集でメモを変える・消す
        store
            .update(&b, "いつもお世話になっております。", "メールの書き出し")
            .unwrap();
        assert_eq!(store.entries()[1].memo, "メールの書き出し");
        store.update(&a, "〒100-0001 東京都千代田区…", "").unwrap();
        assert_eq!(store.entries()[0].memo, "");
        assert!(!memo_path(&a).exists());
        // 消すとメモも消える。同じ番号に新しく足したものに古いメモは残らない
        store.remove(&b).unwrap();
        assert!(!memo_path(&b).exists());
        fs::write(memo_path(&b), "消し残し").unwrap();
        assert_eq!(store.add("新しい本文", "").unwrap(), b);
        assert_eq!(store.entries()[1].memo, "");
        assert_eq!(clean_memo(&"長".repeat(500)).chars().count(), MEMO_LIMIT);
        // 前の版の定型文（メモのファイルがない）もそのまま読める
        assert_eq!(store.entries().len(), 2);
        fs::remove_dir_all(dir).unwrap();
    }
}
