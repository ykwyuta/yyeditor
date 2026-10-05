//! 開いたファイルの履歴とブックマーク（上限つきのパスの一覧）。
//!
//! 設定フォルダ（[`crate::config_dir`]）に 1 行 1 パス（UTF-8）で保存する。複数の yyeditor が
//! 同時に動いていても互いの変更を消さないよう、変更するたびにファイルを読み直してから
//! 書き換える（[`PathList::update`]）。書き込みは一時ファイルに書いてから置き換える。

use std::io;
use std::path::{Path, PathBuf};

/// 履歴の上限。
pub const HISTORY_LIMIT: usize = 1000;
/// ブックマークの上限。
pub const BOOKMARK_LIMIT: usize = 100;

/// 保存先のファイル名。
pub const HISTORY_FILE: &str = "history.txt";
pub const BOOKMARK_FILE: &str = "bookmarks.txt";

/// 上限つきのパスの一覧。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PathList {
    items: Vec<PathBuf>,
    limit: usize,
}

/// 2 つのパスが同じファイルを指すか（Windows では大文字・小文字と区切りの `/` `\` を区別しない）。
pub fn same_path(a: &Path, b: &Path) -> bool {
    if cfg!(windows) {
        let norm = |p: &Path| p.to_string_lossy().replace('/', "\\").to_lowercase();
        norm(a) == norm(b)
    } else {
        a == b
    }
}

impl PathList {
    pub fn new(limit: usize) -> PathList {
        PathList {
            items: Vec::new(),
            limit: limit.max(1),
        }
    }

    /// 文字列（1 行 1 パス）から読む。空行と重複は除き、上限を超えた分は捨てる。
    pub fn parse(text: &str, limit: usize) -> PathList {
        let mut list = PathList::new(limit);
        for line in text.lines() {
            let line = line.trim_end_matches('\r');
            if line.is_empty() {
                continue;
            }
            let p = PathBuf::from(line);
            if !list.contains(&p) && list.items.len() < list.limit {
                list.items.push(p);
            }
        }
        list
    }

    /// 保存する文字列。
    pub fn to_text(&self) -> String {
        let mut s = String::new();
        for p in &self.items {
            s += &p.to_string_lossy();
            s.push('\n');
        }
        s
    }

    /// ファイルから読む（なければ空）。
    pub fn load(path: &Path, limit: usize) -> PathList {
        match std::fs::read(path) {
            Ok(b) => PathList::parse(&String::from_utf8_lossy(&b), limit),
            Err(_) => PathList::new(limit),
        }
    }

    /// ファイルに書く（一時ファイルに書いてから置き換える）。
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, self.to_text())?;
        std::fs::rename(&tmp, path)
    }

    /// ファイルを読み直して `f` で変更し、変わっていれば書く。変更後の一覧を返す。
    pub fn update<R>(
        path: &Path,
        limit: usize,
        f: impl FnOnce(&mut PathList) -> R,
    ) -> io::Result<(PathList, R)> {
        let mut list = PathList::load(path, limit);
        // 書いてある文字列で比べる（Windows の `Path` の比較は区切りの `/` と `\` を区別しないので、
        // 区切りだけを直した変更も書き込むため）
        let before = list.to_text();
        let r = f(&mut list);
        if list.to_text() != before {
            list.save(path)?;
        }
        Ok((list, r))
    }

    pub fn items(&self) -> &[PathBuf] {
        &self.items
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn limit(&self) -> usize {
        self.limit
    }

    pub fn contains(&self, p: &Path) -> bool {
        self.items.iter().any(|q| same_path(q, p))
    }

    /// 先頭に置く（既にあれば先頭へ移す）。上限を超えたら末尾（最も古いもの）を捨てる。
    pub fn touch(&mut self, p: &Path) {
        self.items.retain(|q| !same_path(q, p));
        self.items.insert(0, p.to_owned());
        self.items.truncate(self.limit);
    }

    /// 末尾に加える。既にあれば `Ok(false)`、上限に達していれば `Err(())`。
    #[allow(clippy::result_unit_err)]
    pub fn push(&mut self, p: &Path) -> Result<bool, ()> {
        if self.contains(p) {
            return Ok(false);
        }
        if self.items.len() >= self.limit {
            return Err(());
        }
        self.items.push(p.to_owned());
        Ok(true)
    }

    /// 取り除く。あれば `true`。
    pub fn remove(&mut self, p: &Path) -> bool {
        let n = self.items.len();
        self.items.retain(|q| !same_path(q, p));
        self.items.len() != n
    }

    /// `p` を 1 つ前（`up`）・後ろへ動かす。動かせたら `true`。
    pub fn shift(&mut self, p: &Path, up: bool) -> bool {
        let Some(i) = self.items.iter().position(|q| same_path(q, p)) else {
            return false;
        };
        let j = if up {
            match i.checked_sub(1) {
                Some(j) => j,
                None => return false,
            }
        } else if i + 1 < self.items.len() {
            i + 1
        } else {
            return false;
        };
        self.items.swap(i, j);
        true
    }
}

/// 設定フォルダの中の一覧のファイル。
pub fn list_file(name: &str) -> Option<PathBuf> {
    crate::config_dir().map(|d| d.join(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn history_keeps_most_recent_first_up_to_the_limit() {
        let mut h = PathList::new(3);
        for f in ["a", "b", "c", "d"] {
            h.touch(&p(f));
        }
        assert_eq!(h.items(), [p("d"), p("c"), p("b")]);
        h.touch(&p("b"));
        assert_eq!(h.items(), [p("b"), p("d"), p("c")]);
        assert!(h.remove(&p("d")));
        assert!(!h.remove(&p("d")));
        assert_eq!(h.items(), [p("b"), p("c")]);
    }

    #[test]
    fn bookmarks_refuse_duplicates_and_overflow() {
        let mut b = PathList::new(2);
        assert_eq!(b.push(&p("x")), Ok(true));
        assert_eq!(b.push(&p("x")), Ok(false));
        assert_eq!(b.push(&p("y")), Ok(true));
        assert_eq!(b.push(&p("z")), Err(()));
        assert!(b.shift(&p("y"), true));
        assert_eq!(b.items(), [p("y"), p("x")]);
        assert!(!b.shift(&p("y"), true));
        assert!(!b.shift(&p("x"), false));
    }

    #[test]
    fn parses_and_saves_one_path_per_line() {
        let l = PathList::parse("C:\\a.txt\r\n\r\n/b c.txt\nC:\\a.txt\n", 10);
        assert_eq!(l.items(), [p("C:\\a.txt"), p("/b c.txt")]);
        assert_eq!(l.to_text(), "C:\\a.txt\n/b c.txt\n");
        // 上限を超えた分は読まない
        assert_eq!(PathList::parse("1\n2\n3\n", 2).len(), 2);
    }

    #[test]
    fn update_rereads_the_file_so_other_instances_are_kept() {
        let dir = std::env::temp_dir().join(format!("yy-recent-{}", std::process::id()));
        let file = dir.join("history.txt");
        let _ = std::fs::remove_dir_all(&dir);
        let (l, _) = PathList::update(&file, 5, |l| l.touch(&p("one"))).unwrap();
        assert_eq!(l.items(), [p("one")]);
        // 別のプロセスが書いた内容
        std::fs::write(&file, "other\none\n").unwrap();
        let (l, _) = PathList::update(&file, 5, |l| l.touch(&p("two"))).unwrap();
        assert_eq!(l.items(), [p("two"), p("other"), p("one")]);
        assert_eq!(PathList::load(&file, 5), l);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
