//! 定義の一覧とファイル種類の判定（10 章 3）。
//!
//! 組み込みの定義は実行ファイルにテキストで埋め込み、使うときに初めてコンパイルする
//! （起動時間に影響させない）。利用者の定義（フォルダ内の `*.toml`）は同じ名前の組み込み定義を
//! 置き換える。

use std::path::Path;
use std::sync::{Arc, OnceLock};

use regex_automata::meta::Regex;

use crate::Syntax;
use crate::def::{DefFile, build, parse_def};

macro_rules! builtin {
    ($($id:literal),* $(,)?) => {
        &[$(($id, include_str!(concat!("../syntaxes/", $id, ".toml")))),*]
    };
}

/// 組み込みの定義（id, TOML）。
static BUILTIN: &[(&str, &str)] = builtin!(
    "c",
    "cpp",
    "csharp",
    "java",
    "rust",
    "go",
    "python",
    "javascript",
    "typescript",
    "php",
    "ruby",
    "perl",
    "vb",
    "kotlin",
    "swift",
    "batch",
    "powershell",
    "shell",
    "html",
    "xml",
    "css",
    "json",
    "yaml",
    "toml",
    "ini",
    "markdown",
    "sql",
    "cobol",
    "cobol-free",
    "jcl",
    "pli",
    "rpg",
    "log",
    "diff",
    "makefile",
    "dockerfile",
    "gitignore",
);

struct Entry {
    id: String,
    name: String,
    extensions: Vec<String>,
    filenames: Vec<String>,
    aliases: Vec<String>,
    first_line: Option<Regex>,
    /// 定義（コンパイルは初めて使うとき）
    def: std::sync::Mutex<Option<DefFile>>,
    compiled: OnceLock<Result<Arc<Syntax>, String>>,
    /// 利用者の定義
    user: bool,
}

/// 定義の一覧。
pub struct Registry {
    entries: Vec<Entry>,
}

impl Default for Registry {
    fn default() -> Self {
        Registry::builtin()
    }
}

fn entry(id: &str, text: &str, user: bool) -> Result<Entry, String> {
    let def = parse_def(text)?;
    let first_line = match &def.first_line {
        Some(p) => Some(Regex::new(p).map_err(|e| format!("first_line: {e}"))?),
        None => None,
    };
    Ok(Entry {
        id: id.to_owned(),
        name: def.name.clone(),
        extensions: def
            .extensions
            .iter()
            .map(|e| e.to_ascii_lowercase())
            .collect(),
        filenames: def.filenames.clone(),
        aliases: def.aliases.iter().map(|a| a.to_ascii_lowercase()).collect(),
        first_line,
        def: std::sync::Mutex::new(Some(def)),
        compiled: OnceLock::new(),
        user,
    })
}

impl Registry {
    /// 組み込みの定義だけの一覧。
    pub fn builtin() -> Registry {
        let entries = BUILTIN
            .iter()
            .map(|(id, text)| {
                entry(id, text, false)
                    .unwrap_or_else(|e| panic!("組み込みの定義 {id} が不正です: {e}"))
            })
            .collect();
        Registry { entries }
    }

    /// フォルダ内の `*.toml` を読み込む（同じ名前の組み込み定義を置き換える）。
    /// 読めなかったファイルの説明を返す。
    pub fn load_dir(&mut self, dir: &Path) -> Vec<String> {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut paths: Vec<_> = rd
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("toml"))
            })
            .collect();
        paths.sort();
        let mut errors = Vec::new();
        for path in paths {
            let id = path
                .file_stem()
                .map(|s| s.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default();
            let r = std::fs::read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(|text| {
                    let e = entry(&id, &text, true)?;
                    // 正規表現の誤りも起動時に知らせる
                    let s = e.compile()?;
                    let _ = e.compiled.set(Ok(s));
                    Ok(e)
                });
            match r {
                Ok(e) => match self.entries.iter().position(|x| x.id == id) {
                    Some(i) => self.entries[i] = e,
                    None => self.entries.push(e),
                },
                Err(e) => errors.push(format!("{}: {e}", path.display())),
            }
        }
        errors
    }

    /// （id, 表示名）の一覧（表示名の順）。
    pub fn list(&self) -> Vec<(String, String)> {
        let mut v: Vec<_> = self
            .entries
            .iter()
            .map(|e| (e.id.clone(), e.name.clone()))
            .collect();
        v.sort_by_key(|(_, n)| n.to_lowercase());
        v
    }

    /// 定義を得る（初めてならコンパイルする）。
    pub fn get(&self, id: &str) -> Result<Arc<Syntax>, String> {
        let e = self
            .find(id)
            .ok_or_else(|| format!("ハイライトの定義 {id} がありません"))?;
        e.compiled.get_or_init(|| e.compile()).clone()
    }

    fn find(&self, name: &str) -> Option<&Entry> {
        let key = name.trim().to_ascii_lowercase();
        self.entries
            .iter()
            .find(|e| e.id == key)
            .or_else(|| self.entries.iter().find(|e| e.aliases.contains(&key)))
            .or_else(|| {
                self.entries
                    .iter()
                    .find(|e| e.name.to_ascii_lowercase() == key)
            })
    }

    /// 利用者が追加・置き換えた定義か。
    pub fn is_user(&self, id: &str) -> bool {
        self.find(id).is_some_and(|e| e.user)
    }

    /// ファイル種類を判定する（10 章 3）: モードライン → ファイル名 → 拡張子（複合拡張子を優先）
    /// → 先頭行のパターン。`head` はファイルの先頭部分。
    pub fn detect(&self, path: Option<&Path>, head: &[u8]) -> Option<String> {
        let head = &head[..head.len().min(4096)];
        if let Some(name) = modeline(head)
            && let Some(e) = self.find(&name)
        {
            return Some(e.id.clone());
        }
        if let Some(file) = path
            .and_then(|p| p.file_name())
            .map(|f| f.to_string_lossy())
        {
            if let Some(e) = self
                .entries
                .iter()
                .find(|e| e.filenames.iter().any(|f| f.eq_ignore_ascii_case(&file)))
            {
                return Some(e.id.clone());
            }
            let lower = file.to_ascii_lowercase();
            // 複合拡張子（.d.ts など）を優先するため、最も長い一致を選ぶ
            let best = self
                .entries
                .iter()
                .flat_map(|e| e.extensions.iter().map(move |x| (e, x)))
                .filter(|(_, x)| lower.ends_with(&format!(".{x}")))
                .max_by_key(|(_, x)| x.len());
            if let Some((e, _)) = best {
                return Some(e.id.clone());
            }
        }
        let first = head.split(|&b| b == b'\n').next().unwrap_or(&[]);
        self.entries
            .iter()
            .find(|e| e.first_line.as_ref().is_some_and(|r| r.is_match(first)))
            .map(|e| e.id.clone())
    }
}

impl Entry {
    fn compile(&self) -> Result<Arc<Syntax>, String> {
        let def = self
            .def
            .lock()
            .map_err(|e| e.to_string())?
            .take()
            .ok_or("定義はすでに使われました")?;
        build(&self.id, def).map(Arc::new)
    }
}

/// 先頭の数行のモードライン（`vim: ft=python`・`-*- mode: ruby -*-`）の種類名。
fn modeline(head: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(head);
    for line in text.lines().take(5) {
        if let Some(i) = line.find("-*-") {
            let rest = &line[i + 3..];
            let body = rest.split("-*-").next().unwrap_or("");
            for part in body.split(';') {
                let part = part.trim();
                if let Some(v) = part
                    .strip_prefix("mode:")
                    .or_else(|| part.strip_prefix("Mode:"))
                {
                    return Some(v.trim().to_owned());
                }
                if !part.contains(':') && !part.is_empty() && !body.contains(';') {
                    return Some(part.to_owned());
                }
            }
        }
        for key in ["vim:", "vi:", "ex:"] {
            if let Some(i) = line.find(key) {
                for tok in line[i + key.len()..].split([' ', ':']) {
                    if let Some(v) = tok
                        .strip_prefix("ft=")
                        .or_else(|| tok.strip_prefix("filetype="))
                        .or_else(|| tok.strip_prefix("syntax="))
                    {
                        return Some(v.trim().to_owned());
                    }
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modelines() {
        assert_eq!(modeline(b"# vim: ft=python\n").as_deref(), Some("python"));
        assert_eq!(
            modeline(b"# vim: set ts=4 filetype=ruby :\n").as_deref(),
            Some("ruby")
        );
        assert_eq!(
            modeline(b"// -*- mode: c++; tab-width: 4 -*-\n").as_deref(),
            Some("c++")
        );
        assert_eq!(modeline(b"/* -*- perl -*- */\n").as_deref(), Some("perl"));
        assert_eq!(modeline(b"plain text\n"), None);
    }
}
