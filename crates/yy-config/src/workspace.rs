//! ワークスペース（起点のフォルダの一覧）。
//!
//! VS Code のワークスペースと同じく、複数のフォルダをまとめて扱う。ファイル（`*.yyworkspace`、
//! TOML）に保存する。
//!
//! ```toml
//! folders = ["C:\\src\\app", "..\\docs", "ssh://build/home/yamada/proj"]
//! ```
//!
//! 相対パスはワークスペースのファイルのあるフォルダからの位置。`ssh://` で始まるものは
//! SSH 接続先のフォルダ（11 章）で、書いたとおりに扱う。名前を付けて保存していない
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
/// ターミナル（yyterm）の、名前を付けていないワークスペースのファイル名。
pub const TERMINAL_UNTITLED_FILE: &str = "terminal-untitled.yyworkspace";
/// ターミナル（yyterm）が最後に使ったワークスペースを記録するファイル名。
pub const TERMINAL_LAST_FILE: &str = "terminal-last-workspace.txt";
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
            if crate::recent::is_remote(&p) {
                ws.add(&p);
                continue;
            }
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

/// 束ねるフォルダの深さの上限
pub const COMPACT_DEPTH: usize = 32;
/// 手元のフォルダで、先に中身を調べて束ねるフォルダの数の上限（多いフォルダで遅くしない）
pub const COMPACT_LIMIT: usize = 200;

/// VS Code の「フォルダを束ねる」（Compact Folders）: フォルダ `entry` の中身がフォルダ 1 つだけなら、
/// 名前を `a/b` とつなげてパスをその中のフォルダにする（続く限り）。`list` はフォルダの中身を返す
/// （読めなければ `None` で止める）。束ねたら `true`。
pub fn compact(entry: &mut Entry, mut list: impl FnMut(&Path) -> Option<Vec<Entry>>) -> bool {
    if !entry.is_dir {
        return false;
    }
    let mut merged = false;
    for _ in 0..COMPACT_DEPTH {
        let Some(children) = list(&entry.path) else {
            break;
        };
        match children.as_slice() {
            [only] if only.is_dir => {
                entry.name = format!("{}/{}", entry.name, only.name);
                entry.path = only.path.clone();
                merged = true;
            }
            _ => break,
        }
    }
    merged
}

/// 手元のフォルダの項目を束ねる（[`compact`]）。束ねる前のパス（束ねた並びの先頭）を項目ごとに返す。
/// フォルダが [`COMPACT_LIMIT`] を超える分は調べない。
pub fn compact_local(entries: &mut [Entry]) -> Vec<PathBuf> {
    let mut heads = Vec::with_capacity(entries.len());
    let mut checked = 0;
    for e in entries.iter_mut() {
        heads.push(e.path.clone());
        if e.is_dir && checked < COMPACT_LIMIT {
            checked += 1;
            compact(e, |p| list_dir(p).ok().map(|(v, _)| v));
        }
    }
    heads
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
    last_used_in(LAST_FILE)
}

/// 最後に使ったワークスペースを記録する。
pub fn set_last_used(path: &Path) -> io::Result<()> {
    set_last_used_in(LAST_FILE, path)
}

/// 記録のファイル `record`（設定フォルダの中）にある、最後に使ったワークスペース。
pub fn last_used_in(record: &str) -> Option<PathBuf> {
    let text = std::fs::read_to_string(config_file(record)?).ok()?;
    let line = text.lines().next()?.trim();
    (!line.is_empty()).then(|| PathBuf::from(line))
}

/// 最後に使ったワークスペースを `record` に記録する。
pub fn set_last_used_in(record: &str, path: &Path) -> io::Result<()> {
    let Some(file) = config_file(record) else {
        return Ok(());
    };
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(file, format!("{}\n", path.to_string_lossy()))
}

// ---- ファイル・フォルダの操作に使うパスの計算 ------------------------------------
//
// ワークスペースの項目のパスは、手元のパスか `ssh://接続先/パス`（11 章）のどちらか。
// リモートの場所は Windows の区切り（`\`）を使わずに文字列のまま扱う。

fn remote_parts(path: &Path) -> Option<(&str, &str)> {
    let s = path.to_str()?;
    let rest = s.strip_prefix("ssh://")?;
    let slash = rest.find('/')?;
    // （接続先, 接続先の中の絶対パス）
    Some((&s[..6 + slash], &rest[slash..]))
}

/// ファイル・フォルダの名前として使えるか。`remote` なら接続先（Linux）の規則、でなければ Windows の規則。
pub fn check_name(name: &str, remote: bool) -> Result<(), String> {
    if name.is_empty() || name.trim().is_empty() {
        return Err("名前を入力してください".into());
    }
    if name == "." || name == ".." {
        return Err(format!("「{name}」は名前に使えません"));
    }
    let bad: &[char] = if remote {
        &['/', '\0']
    } else {
        &['/', '\\', ':', '*', '?', '"', '<', '>', '|']
    };
    if let Some(c) = name.chars().find(|c| bad.contains(c) || c.is_control()) {
        return Err(format!("名前に「{}」は使えません", c.escape_default()));
    }
    if !remote {
        if name.ends_with(['.', ' ']) {
            return Err("名前の最後に「.」や空白は使えません".into());
        }
        let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
        let reserved = ["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str())
            || ((stem.starts_with("COM") || stem.starts_with("LPT"))
                && stem.len() == 4
                && stem.as_bytes()[3].is_ascii_digit());
        if reserved {
            return Err(format!("「{name}」は Windows では名前に使えません"));
        }
    }
    Ok(())
}

/// `dir` の中の `name`。
pub fn child(dir: &Path, name: &str) -> PathBuf {
    match dir.to_str().filter(|_| remote_parts(dir).is_some()) {
        Some(s) => PathBuf::from(format!("{}/{name}", s.trim_end_matches('/'))),
        None => dir.join(name),
    }
}

/// 親のフォルダ（ルート・接続先のルートなら `None`）。
pub fn parent(path: &Path) -> Option<PathBuf> {
    match remote_parts(path) {
        Some((target, p)) => {
            let p = p.trim_end_matches('/');
            let i = p.rfind('/')?;
            Some(PathBuf::from(format!(
                "{target}{}",
                if i == 0 { "/" } else { &p[..i] }
            )))
        }
        None => path.parent().map(|p| p.to_owned()),
    }
}

/// 最後の名前。
pub fn name_of(path: &Path) -> String {
    match remote_parts(path) {
        Some((_, p)) => p
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("")
            .to_owned(),
        None => path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
    }
}

/// 同じ場所（同じパソコン、または同じ接続先）か。移動はその中だけで行う。
pub fn same_place(a: &Path, b: &Path) -> bool {
    match (remote_parts(a), remote_parts(b)) {
        (Some((x, _)), Some((y, _))) => x.eq_ignore_ascii_case(y),
        (None, None) => true,
        _ => false,
    }
}

/// `path` の `dir` からの残り（`path` が `dir` 自身なら空、`dir` の外なら `None`）。
fn rest_after(path: &Path, dir: &Path) -> Option<Vec<String>> {
    match (remote_parts(path), remote_parts(dir)) {
        (Some((t, p)), Some((u, d))) => {
            if !t.eq_ignore_ascii_case(u) {
                return None;
            }
            let split = |s: &str| -> Vec<String> {
                s.split('/')
                    .filter(|c| !c.is_empty())
                    .map(str::to_owned)
                    .collect()
            };
            let (p, d) = (split(p), split(d));
            p.starts_with(&d).then(|| p[d.len()..].to_vec())
        }
        (None, None) => {
            // Windows は大文字・小文字を区別しない
            let key = |c: std::path::Component| {
                let s = c.as_os_str().to_string_lossy().into_owned();
                if cfg!(windows) { s.to_lowercase() } else { s }
            };
            let p: Vec<_> = path.components().collect();
            let d: Vec<_> = dir.components().collect();
            if d.len() > p.len() || p.iter().zip(&d).any(|(a, b)| key(*a) != key(*b)) {
                return None;
            }
            Some(
                p[d.len()..]
                    .iter()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect(),
            )
        }
        _ => None,
    }
}

/// `path` が `dir` 自身か、その中か。
pub fn is_within(path: &Path, dir: &Path) -> bool {
    rest_after(path, dir).is_some()
}

/// `from` を `to` に移したとき、`path`（`from` 自身またはその中）の移した先。`from` の外なら `None`。
pub fn relocated(path: &Path, from: &Path, to: &Path) -> Option<PathBuf> {
    let rest = rest_after(path, from)?;
    Some(rest.iter().fold(to.to_owned(), |p, name| child(&p, name)))
}

/// コピーの名前（`n` 番目の候補）。`n == 0` は元の名前、1 は「名前 - コピー.拡張子」、2 以降は
/// 「名前 - コピー (n).拡張子」（エクスプローラーと同じ形）。フォルダには拡張子を考えない。
pub fn copy_name(name: &str, n: usize, is_dir: bool) -> String {
    if n == 0 {
        return name.to_owned();
    }
    let suffix = if n == 1 {
        " - コピー".to_owned()
    } else {
        format!(" - コピー ({n})")
    };
    // 先頭の「.」だけのもの（.bashrc）は拡張子とみなさない
    match name.rfind('.').filter(|&i| !is_dir && i > 0) {
        Some(i) => format!("{}{suffix}{}", &name[..i], &name[i..]),
        None => format!("{name}{suffix}"),
    }
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
    fn keeps_remote_folders_as_written() {
        let base = root().join("ws");
        let remote = "ssh://yamada@build:2222/home/yamada/proj";
        let toml = format!("folders = [\"docs\", {remote:?}]");
        let ws = Workspace::parse(&toml, &base).unwrap();
        assert_eq!(ws.folders, [base.join("docs"), PathBuf::from(remote)]);
        let text = ws.to_toml(Some(&base));
        assert!(text.contains(remote), "{text}");
        assert_eq!(Workspace::parse(&text, &base).unwrap(), ws);
        let file = PathBuf::from(format!("{remote}/src/main.rs"));
        assert_eq!(ws.root_of(&file), Some(Path::new(remote)));
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

    #[test]
    fn checks_names() {
        assert!(check_name("a.txt", false).is_ok());
        assert!(check_name("日本語 フォルダ", false).is_ok());
        for bad in [
            "", "  ", ".", "..", "a/b", "a\\b", "a:b", "a?", "con", "COM1.txt", "a.",
        ] {
            assert!(check_name(bad, false).is_err(), "{bad:?}");
        }
        assert!(check_name("a:b?", true).is_ok());
        assert!(check_name("a/b", true).is_err());
        assert!(check_name("..", true).is_err());
    }

    #[test]
    fn remote_paths() {
        let d = Path::new("ssh://u@h:22/home/u/proj");
        assert_eq!(
            child(d, "a.txt"),
            PathBuf::from("ssh://u@h:22/home/u/proj/a.txt")
        );
        assert_eq!(
            child(Path::new("ssh://h/"), "etc"),
            PathBuf::from("ssh://h/etc")
        );
        assert_eq!(parent(d), Some(PathBuf::from("ssh://u@h:22/home/u")));
        assert_eq!(
            parent(Path::new("ssh://h/etc")),
            Some(PathBuf::from("ssh://h/"))
        );
        assert_eq!(parent(Path::new("ssh://h/")), None);
        assert_eq!(name_of(d), "proj");
        let f = child(d, "src/main.rs");
        assert!(is_within(&f, d) && is_within(d, d));
        assert!(!is_within(Path::new("ssh://u@h:22/home/u/project"), d));
        assert!(!is_within(Path::new("ssh://other/home/u/proj/x"), d));
        assert_eq!(
            relocated(&f, d, Path::new("ssh://u@h:22/srv/p")),
            Some(PathBuf::from("ssh://u@h:22/srv/p/src/main.rs"))
        );
        assert!(same_place(d, Path::new("ssh://U@H:22/x")));
        assert!(!same_place(d, Path::new("ssh://other/x")));
        assert!(!same_place(d, &root().join("x")));
    }

    #[test]
    fn local_paths() {
        let r = root();
        let d = r.join("work").join("proj");
        let f = d.join("src").join("main.rs");
        assert!(is_within(&f, &d));
        assert!(!is_within(&r.join("work").join("project"), &d));
        assert_eq!(
            relocated(&f, &d, &r.join("other")),
            Some(r.join("other").join("src").join("main.rs"))
        );
        assert_eq!(relocated(&d, &d, &r.join("x")), Some(r.join("x")));
        assert_eq!(relocated(&r.join("y"), &d, &r.join("x")), None);
        assert_eq!(parent(&f), Some(d.join("src")));
        assert_eq!(name_of(&f), "main.rs");
        assert!(same_place(&f, &r.join("z")));
    }

    #[test]
    fn copy_names() {
        assert_eq!(copy_name("a.txt", 0, false), "a.txt");
        assert_eq!(copy_name("a.txt", 1, false), "a - コピー.txt");
        assert_eq!(copy_name("a.tar.gz", 2, false), "a.tar - コピー (2).gz");
        assert_eq!(copy_name(".bashrc", 1, false), ".bashrc - コピー");
        assert_eq!(copy_name("v1.2", 1, true), "v1.2 - コピー");
    }
}

#[cfg(test)]
mod compact_tests {
    use super::*;

    #[test]
    fn compacts_single_folder_chains_like_vscode() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        // a/b/c/（ファイル 2 つ）、x/（フォルダ 1 つとファイル 1 つ）、y/z/（空）、e/（空）
        std::fs::create_dir_all(d.join("a/b/c")).unwrap();
        std::fs::write(d.join("a/b/c/1.txt"), b"").unwrap();
        std::fs::write(d.join("a/b/c/2.txt"), b"").unwrap();
        std::fs::create_dir_all(d.join("x/inner")).unwrap();
        std::fs::write(d.join("x/f.txt"), b"").unwrap();
        std::fs::create_dir_all(d.join("y/z")).unwrap();
        std::fs::create_dir_all(d.join("e")).unwrap();
        std::fs::write(d.join("top.txt"), b"").unwrap();
        let (mut entries, _) = list_dir(d).unwrap();
        let heads = compact_local(&mut entries);
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["a/b/c", "e", "x", "y/z", "top.txt"]);
        assert_eq!(entries[0].path, d.join("a").join("b").join("c"));
        assert_eq!(heads[0], d.join("a"));
        assert_eq!(entries[3].path, d.join("y").join("z"));
        assert_eq!(heads[3], d.join("y"));
        assert_eq!(heads[2], entries[2].path);
        // ファイルは束ねない
        let mut f = entries[4].clone();
        assert!(!compact(&mut f, |_| unreachable!()));
    }
}
