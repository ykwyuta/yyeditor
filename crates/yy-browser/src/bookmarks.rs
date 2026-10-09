//! ブックマーク（設定のフォルダの `bookmarks.toml`。19 章 4.1）。
//!
//! プロファイルに関係なく共通。フォルダは名前（入れ子は `親 / 子`）で持つ。Edge・Chrome・Firefox が
//! 書き出す HTML（Netscape Bookmark File）を読み込み・書き出しできる。

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// フォルダの入れ子の区切り。
pub const FOLDER_SEP: &str = " / ";

/// ブックマーク 1 つ。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bookmark {
    pub title: String,
    pub url: String,
    /// フォルダ（空なら一番上。入れ子は `親 / 子`）
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub folder: String,
}

impl Bookmark {
    pub fn new(title: &str, url: &str, folder: &str) -> Bookmark {
        Bookmark {
            title: title.trim().to_owned(),
            url: url.trim().to_owned(),
            folder: normalize_folder(folder),
        }
    }

    /// 表示する名前（題名がなければ URL）。
    pub fn label(&self) -> &str {
        if self.title.trim().is_empty() {
            &self.url
        } else {
            &self.title
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        let u = self.url.trim();
        if u.is_empty() {
            return Err("URL を入れてください".into());
        }
        if u.chars().any(char::is_control) || self.title.chars().any(|c| c == '\n' || c == '\r') {
            return Err("名前・URL に改行は使えません".into());
        }
        if u.to_ascii_lowercase().starts_with("javascript:") {
            return Err("javascript: の URL はブックマークにできません".into());
        }
        Ok(())
    }
}

/// フォルダの名前をそろえる（区切りの前後の空白・空の段を除く）。
pub fn normalize_folder(f: &str) -> String {
    f.split('/')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(FOLDER_SEP)
}

/// ブックマークの一覧。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bookmarks {
    #[serde(default, rename = "bookmark")]
    pub items: Vec<Bookmark>,
}

impl Bookmarks {
    /// 読む（なければ空）。
    pub fn load(path: &Path) -> io::Result<Bookmarks> {
        match std::fs::read_to_string(path) {
            Ok(t) => toml::from_str(&t).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Bookmarks::default()),
            Err(e) => Err(e),
        }
    }

    /// 保存する（一時ファイルから置き換える）。
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let text = toml::to_string_pretty(self).map_err(io::Error::other)?;
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path)
    }

    /// その URL のブックマーク（なければ `None`）。
    pub fn find(&self, url: &str) -> Option<usize> {
        let u = url.trim();
        self.items.iter().position(|b| b.url == u)
    }

    /// 足す（同じ URL があれば置き換える）。番号を返す。
    pub fn put(&mut self, b: Bookmark) -> usize {
        match self.find(&b.url) {
            Some(i) => {
                self.items[i] = b;
                i
            }
            None => {
                self.items.push(b);
                self.items.len() - 1
            }
        }
    }

    /// 一つ上・下へ動かす。動かした先の番号を返す。
    pub fn shift(&mut self, i: usize, up: bool) -> Option<usize> {
        let j = if up { i.checked_sub(1)? } else { i + 1 };
        if j >= self.items.len() || i >= self.items.len() {
            return None;
        }
        self.items.swap(i, j);
        Some(j)
    }

    /// 使われているフォルダ（出てきた順）。
    pub fn folders(&self) -> Vec<String> {
        let mut v: Vec<String> = Vec::new();
        for b in &self.items {
            if !b.folder.is_empty() && !v.contains(&b.folder) {
                v.push(b.folder.clone());
            }
        }
        v
    }

    /// メニューに出す木（フォルダ → 中身）。
    pub fn tree(&self) -> Node {
        let mut root = Node::default();
        for (i, b) in self.items.iter().enumerate() {
            let mut n = &mut root;
            for part in b.folder.split(FOLDER_SEP).filter(|p| !p.is_empty()) {
                let k = match n.folders.iter().position(|(name, _)| name == part) {
                    Some(k) => k,
                    None => {
                        n.folders.push((part.to_owned(), Node::default()));
                        n.folders.len() - 1
                    }
                };
                n = &mut n.folders[k].1;
            }
            n.items.push(i);
        }
        root
    }

    /// HTML（Netscape Bookmark File）から足す。同じ URL のものは足さない。足した数を返す。
    pub fn import_html(&mut self, html: &str) -> usize {
        let mut added = 0;
        for b in parse_html(html) {
            if self.find(&b.url).is_none() {
                self.items.push(b);
                added += 1;
            }
        }
        added
    }

    /// HTML（Netscape Bookmark File）に書き出す。
    pub fn export_html(&self) -> String {
        let mut s = String::from(
            "<!DOCTYPE NETSCAPE-Bookmark-file-1>\n\
             <META HTTP-EQUIV=\"Content-Type\" CONTENT=\"text/html; charset=UTF-8\">\n\
             <TITLE>Bookmarks</TITLE>\n<H1>Bookmarks</H1>\n",
        );
        write_node(&mut s, self, &self.tree(), 0);
        s
    }
}

/// フォルダの木の 1 段。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Node {
    /// 下のフォルダ（名前, 中身）
    pub folders: Vec<(String, Node)>,
    /// この段のブックマーク（`items` の番号）
    pub items: Vec<usize>,
}

fn write_node(s: &mut String, list: &Bookmarks, n: &Node, depth: usize) {
    let pad = "    ".repeat(depth);
    s.push_str(&format!("{pad}<DL><p>\n"));
    for (name, child) in &n.folders {
        s.push_str(&format!("{pad}    <DT><H3>{}</H3>\n", escape(name)));
        write_node(s, list, child, depth + 1);
    }
    for &i in &n.items {
        let b = &list.items[i];
        s.push_str(&format!(
            "{pad}    <DT><A HREF=\"{}\">{}</A>\n",
            escape(&b.url),
            escape(b.label())
        ));
    }
    s.push_str(&format!("{pad}</DL><p>\n"));
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest.find(';').filter(|e| *e <= 10) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let ent = &rest[1..end];
        let ch = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            _ => ent
                .strip_prefix("#x")
                .or_else(|| ent.strip_prefix("#X"))
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| ent.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// 属性の値（`HREF="…"`。大文字・小文字を区別しない）。
fn attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let key = format!("{}=", name.to_ascii_lowercase());
    let mut from = 0;
    while let Some(i) = lower[from..].find(&key) {
        let at = from + i;
        // 名前の前が区切り（空白）であること
        if at > 0 && !lower.as_bytes()[at - 1].is_ascii_whitespace() {
            from = at + key.len();
            continue;
        }
        let v = &tag[at + key.len()..];
        let (q, body) = match v.chars().next()? {
            c @ ('"' | '\'') => (c, &v[1..]),
            _ => {
                let end = v
                    .find(|c: char| c.is_whitespace() || c == '>')
                    .unwrap_or(v.len());
                return Some(unescape(&v[..end]));
            }
        };
        let end = body.find(q)?;
        return Some(unescape(&body[..end]));
    }
    None
}

/// HTML（Netscape Bookmark File）を読む。フォルダの入れ子は `親 / 子`。javascript: は除く。
pub fn parse_html(html: &str) -> Vec<Bookmark> {
    let mut out = Vec::new();
    // 開いている DL ごとのフォルダの名前（一番外の DL は名前なし）
    let mut stack: Vec<Option<String>> = Vec::new();
    let mut pending: Option<String> = None;
    let lower = html.to_ascii_lowercase();
    let mut pos = 0;
    while let Some(i) = lower[pos..].find('<') {
        let start = pos + i;
        let Some(close) = lower[start..].find('>') else {
            break;
        };
        let end = start + close + 1;
        let tag = &html[start..end];
        let ltag = &lower[start..end];
        pos = end;
        if ltag.starts_with("<h3") {
            let Some(e) = lower[end..].find("</h3") else {
                continue;
            };
            pending = Some(unescape(html[end..end + e].trim()));
            pos = end + e;
        } else if ltag.starts_with("<dl") {
            stack.push(pending.take());
        } else if ltag.starts_with("</dl") {
            stack.pop();
        } else if ltag.starts_with("<a ") || ltag.starts_with("<a\t") {
            let Some(url) = attr(tag, "href") else {
                continue;
            };
            let e = lower[end..].find("</a").unwrap_or(0);
            let title = unescape(html[end..end + e].trim());
            pos = end + e;
            if url.trim().to_ascii_lowercase().starts_with("javascript:") || url.trim().is_empty() {
                continue;
            }
            let folder: Vec<&str> = stack.iter().flatten().map(String::as_str).collect();
            out.push(Bookmark::new(&title, &url, &folder.join(FOLDER_SEP)));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_finds_and_moves() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("bookmarks.toml");
        let mut b = Bookmarks::load(&p).unwrap();
        assert!(b.items.is_empty());
        b.put(Bookmark::new("Example", "https://example.com/", ""));
        b.put(Bookmark::new(
            "社内",
            "https://intra.example.jp/",
            " 仕事 /  社内 ",
        ));
        assert_eq!(b.items[1].folder, "仕事 / 社内");
        // 同じ URL は置き換え
        assert_eq!(
            b.put(Bookmark::new("Example!", "https://example.com/", "")),
            0
        );
        assert_eq!(b.items.len(), 2);
        assert_eq!(b.find(" https://example.com/ "), Some(0));
        assert_eq!(b.shift(0, true), None);
        assert_eq!(b.shift(0, false), Some(1));
        assert_eq!(b.items[1].title, "Example!");
        b.save(&p).unwrap();
        assert_eq!(Bookmarks::load(&p).unwrap(), b);
        assert_eq!(b.folders(), ["仕事 / 社内"]);
        assert!(
            Bookmark::new("x", "javascript:alert(1)", "")
                .validate()
                .is_err()
        );
        assert!(Bookmark::new("x", "", "").validate().is_err());
        assert_eq!(Bookmark::new("", "https://a/", "").label(), "https://a/");
    }

    #[test]
    fn builds_a_tree() {
        let mut b = Bookmarks::default();
        b.put(Bookmark::new("a", "https://a/", "仕事"));
        b.put(Bookmark::new("top", "https://top/", ""));
        b.put(Bookmark::new("b", "https://b/", "仕事 / 社内"));
        b.put(Bookmark::new("c", "https://c/", "仕事"));
        let t = b.tree();
        assert_eq!(t.items, [1]);
        assert_eq!(t.folders.len(), 1);
        let (name, work) = &t.folders[0];
        assert_eq!(name, "仕事");
        assert_eq!(work.items, [0, 3]);
        assert_eq!(work.folders[0].0, "社内");
        assert_eq!(work.folders[0].1.items, [2]);
    }

    #[test]
    fn imports_and_exports_netscape_html() {
        let html = r#"<!DOCTYPE NETSCAPE-Bookmark-file-1>
<META HTTP-EQUIV="Content-Type" CONTENT="text/html; charset=UTF-8">
<TITLE>Bookmarks</TITLE>
<H1>Bookmarks</H1>
<DL><p>
    <DT><H3 ADD_DATE="1" PERSONAL_TOOLBAR_FOLDER="true">お気に入りバー</H3>
    <DL><p>
        <DT><A HREF="https://www.example.com/?a=1&amp;b=2" ADD_DATE="1" ICON="data:x">Example &amp; Co</A>
        <DT><H3>社内</H3>
        <DL><p>
            <DT><a href='https://intra.example.jp/'>イントラ</a>
        </DL><p>
        <DT><A HREF="javascript:void(0)">bookmarklet</A>
    </DL><p>
    <DT><A HREF="https://top.example/">&#x30C8;&#12483;プ</A>
</DL><p>
"#;
        let v = parse_html(html);
        assert_eq!(
            v,
            [
                Bookmark::new(
                    "Example & Co",
                    "https://www.example.com/?a=1&b=2",
                    "お気に入りバー"
                ),
                Bookmark::new(
                    "イントラ",
                    "https://intra.example.jp/",
                    "お気に入りバー / 社内"
                ),
                Bookmark::new("トップ", "https://top.example/", ""),
            ]
        );
        let mut b = Bookmarks::default();
        assert_eq!(b.import_html(html), 3);
        assert_eq!(b.import_html(html), 0); // 同じ URL は足さない
        // 書き出したものを読み直すと同じ
        let out = b.export_html();
        assert!(out.contains("<DT><H3>お気に入りバー</H3>"), "{out}");
        assert!(out.contains("?a=1&amp;b=2"), "{out}");
        // 書き出しはフォルダを先に並べるので、順番は比べない
        let mut back = parse_html(&out);
        let mut orig = b.items.clone();
        back.sort_by(|x, y| x.url.cmp(&y.url));
        orig.sort_by(|x, y| x.url.cmp(&y.url));
        assert_eq!(back, orig);
        assert_eq!(unescape("a &amp b &unknown; c"), "a &amp b &unknown; c");
    }
}
