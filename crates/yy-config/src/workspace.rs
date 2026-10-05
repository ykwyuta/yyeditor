//! ワークスペース（起点のフォルダの一覧）。
//!
//! VS Code のワークスペースと同じく、複数のフォルダをまとめて扱う。ファイル（`*.yyworkspace`、
//! TOML）に保存する。
//!
//! ```toml
//! folders = ["C:\\src\\app", "..\\docs"]
//! ```
//!
//! 相対パスはワークスペースのファイルのあるフォルダからの位置。名前を付けて保存していない
//! ワークスペースは設定フォルダの `untitled.yyworkspace` に保存し、最後に使ったワークスペースは
//! `last-workspace.txt` に記録して次に起動したときに開き直す。

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// ワークスペースのファイルの拡張子。
pub const EXTENSION: &str = "yyworkspace";
/// 名前を付けていないワークスペースのファイル名（設定フォルダの中）。
pub const UNTITLED_FILE: &str = "untitled.yyworkspace";
/// 最後に使ったワークスペースを記録するファイル名（設定フォルダの中）。
pub const LAST_FILE: &str = "last-workspace.txt";
/// 一覧に表示しないフォルダ・ファイル。
pub const EXCLUDED: &[&str] = &[".git", ".svn", ".hg"];
/// 1 つのフォルダに表示する項目の上限。
pub const MAX_ENTRIES: usize = 10_000;

/// ファイルの内容。
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct WorkspaceFile {
    folders: Vec<String>,
}

/// ワークスペース。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Workspace {
    /// 起点のフォルダ（絶対パス、登録した順）
    pub folders: Vec<PathBuf>,
}

impl Workspace {
    /// TOML を読む。相対パスは `base`（ワークスペースのファイルのフォルダ）から解決する。
    pub fn parse(text: &str, base: &Path) -> Result<Workspace, String> {
        let f: WorkspaceFile = toml::from_str(text).map_err(|e| e.to_string())?;
        let mut ws = Workspace::default();
        for s in f.folders {
            let p = PathBuf::from(&s);
            let p = if p.is_absolute() { p } else { base.join(p) };
            ws.add(&normalize(&p));
        }
        Ok(ws)
    }

    /// 保存する TOML（ワークスペースのファイルのフォルダ `base` の中のフォルダは相対パスで書く）。
    pub fn to_toml(&self, base: Option<&Path>) -> String {
        let folders = self
            .folders
            .iter()
            .map(|p| {
                base.and_then(|b| p.strip_prefix(b).ok())
                    .filter(|r| !r.as_os_str().is_empty())
                    .unwrap_or(p)
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        let body = toml::to_string(&WorkspaceFile { folders }).unwrap_or_default();
        format!("# yyeditor のワークスペース\n{body}")
    }

    /// ファイルから読む。
    pub fn load(path: &Path) -> Result<Workspace, String> {
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        Workspace::parse(&text, path.parent().unwrap_or(Path::new("")))
    }

    /// ファイルに書く。
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, self.to_toml(path.parent()))?;
        std::fs::rename(&tmp, path)
    }

    /// フォルダを加える。既にあれば `false`。
    pub fn add(&mut self, folder: &Path) -> bool {
        if self
            .folders
            .iter()
            .any(|f| crate::recent::same_path(f, folder))
        {
            return false;
        }
        self.folders.push(folder.to_owned());
        true
    }

    /// フォルダを取り除く。あれば `true`。
    pub fn remove(&mut self, folder: &Path) -> bool {
        let n = self.folders.len();
        self.folders
            .retain(|f| !crate::recent::same_path(f, folder));
        self.folders.len() != n
    }

    /// `path` を含む起点のフォルダ（最も深いもの）。
    pub fn root_of(&self, path: &Path) -> Option<&Path> {
        self.folders
            .iter()
            .filter(|f| path.starts_with(f))
            .max_by_key(|f| f.components().count())
            .map(|f| f.as_path())
    }
}

/// パスを正規化する（`.` `..` を取り除く。ファイルシステムは調べない）。
pub fn normalize(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            c => out.push(c.as_os_str()),
        }
    }
    out
}

/// フォルダの中の項目。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
}

/// フォルダの中の項目（フォルダが先、それぞれ名前順。大文字・小文字は区別しない）。
/// [`EXCLUDED`] の名前は除く。[`MAX_ENTRIES`] を超えた分は省き、省いた数を返す。
pub fn list_dir(dir: &Path) -> io::Result<(Vec<Entry>, usize)> {
    let mut entries = Vec::new();
    let mut skipped = 0;
    for e in std::fs::read_dir(dir)? {
        let Ok(e) = e else { continue };
        let name = e.file_name().to_string_lossy().into_owned();
        if EXCLUDED.contains(&name.as_str()) {
            continue;
        }
        if entries.len() >= MAX_ENTRIES {
            skipped += 1;
            continue;
        }
        // シンボリックリンク・ジャンクションは指す先の種類で判断する
        let is_dir = e
            .file_type()
            .map(|t| t.is_dir() || (t.is_symlink() && e.path().is_dir()))
            .unwrap_or(false);
        entries.push(Entry {
            name,
            path: e.path(),
            is_dir,
        });
    }
    sort_entries(&mut entries);
    Ok((entries, skipped))
}

/// フォルダを先に、名前順（大文字・小文字を区別せず、数字は数として比べる）に並べる。
pub fn sort_entries(entries: &mut [Entry]) {
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| natural_cmp(&a.name, &b.name))
    });
}

/// 数字の並びを数として比べる（`file2` < `file10`）。大文字・小文字は区別しない。
pub fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(c), Some(d)) if c.is_ascii_digit() && d.is_ascii_digit() => {
                let take = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut s = String::new();
                    while let Some(&c) = it.peek().filter(|c| c.is_ascii_digit()) {
                        s.push(c);
                        it.next();
                    }
                    s
                };
                let (n, m) = (take(&mut x), take(&mut y));
                let (nt, mt) = (n.trim_start_matches('0'), m.trim_start_matches('0'));
                let o = nt.len().cmp(&mt.len()).then_with(|| nt.cmp(mt));
                if o != Ordering::Equal {
                    return o;
                }
            }
            (Some(c), Some(d)) => {
                let o = c.to_lowercase().cmp(d.to_lowercase());
                if o != Ordering::Equal {
                    return o;
                }
                x.next();
                y.next();
            }
        }
    }
}

/// 設定フォルダの中のファイル。
pub fn config_file(name: &str) -> Option<PathBuf> {
    crate::config_dir().map(|d| d.join(name))
}

/// 最後に使ったワークスペースのファイル（記録がなければ `None`）。
pub fn last_used() -> Option<PathBuf> {
    let text = std::fs::read_to_string(config_file(LAST_FILE)?).ok()?;
    let line = text.lines().next()?.trim();
    (!line.is_empty()).then(|| PathBuf::from(line))
}

/// 最後に使ったワークスペースを記録する。
pub fn set_last_used(path: &Path) -> io::Result<()> {
    let Some(file) = config_file(LAST_FILE) else {
        return Ok(());
    };
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(file, format!("{}\n", path.to_string_lossy()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 絶対パスの根（Windows は `C:\`、それ以外は `/`）。
    fn root() -> PathBuf {
        PathBuf::from(if cfg!(windows) { "C:\\" } else { "/" })
    }

    #[test]
    fn parses_and_writes_folders() {
        let r = root();
        let base = r.join("ws");
        let app = r.join("src").join("app");
        let toml = format!(
            "folders = [{:?}, \"docs\", \"../other\", {:?}]",
            app.to_string_lossy(),
            app.to_string_lossy()
        );
        let ws = Workspace::parse(&toml, &base).unwrap();
        assert_eq!(
            ws.folders,
            [app.clone(), base.join("docs"), r.join("other")]
        );
        let text = ws.to_toml(Some(&base));
        assert!(text.contains("\"docs\""), "{text}");
        assert_eq!(Workspace::parse(&text, &base).unwrap(), ws);
        assert!(Workspace::parse("folder = []", &base).is_err());
        assert_eq!(Workspace::parse("", &base).unwrap(), Workspace::default());
    }

    #[test]
    fn adds_removes_and_finds_roots() {
        let r = root();
        let (a, ab) = (r.join("a"), r.join("a").join("b"));
        let mut ws = Workspace::default();
        assert!(ws.add(&a));
        assert!(ws.add(&ab));
        assert!(!ws.add(&a));
        assert_eq!(ws.root_of(&ab.join("c.txt")), Some(ab.as_path()));
        assert_eq!(ws.root_of(&a.join("x.txt")), Some(a.as_path()));
        assert_eq!(ws.root_of(&r.join("z.txt")), None);
        assert!(ws.remove(&a));
        assert_eq!(ws.folders, [ab]);
    }

    #[test]
    fn lists_folders_first_in_natural_order() {
        let dir = std::env::temp_dir().join(format!("yy-ws-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::create_dir_all(dir.join("Docs")).unwrap();
        for f in ["file10.txt", "file2.txt", "README.md", "a.txt"] {
            std::fs::write(dir.join(f), "").unwrap();
        }
        let (entries, skipped) = list_dir(&dir).unwrap();
        let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "Docs",
                "src",
                "a.txt",
                "file2.txt",
                "file10.txt",
                "README.md"
            ]
        );
        assert!(entries[0].is_dir && !entries[2].is_dir);
        assert_eq!(skipped, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saves_relative_to_the_workspace_file() {
        let dir = std::env::temp_dir().join(format!("yy-wss-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join("my.yyworkspace");
        let ws = Workspace {
            folders: vec![dir.join("proj"), root().join("elsewhere")],
        };
        ws.save(&file).unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("\"proj\""), "{text}");
        assert_eq!(Workspace::load(&file).unwrap(), ws);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
