//! Bounded, local-only Markdown transclusion.
use pulldown_cmark::{Event, LinkType, Options, Parser, Tag, TagEnd};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    ops::Range,
    path::{Component, Path, PathBuf},
};
pub const SNIPPET_PREFIX: &str = "__yy_snippets__/";
const MAX_BYTES: usize = 8 * 1024 * 1024;
const MAX_EMBEDS: usize = 256;

pub struct Expansion {
    pub markdown: String,
    pub source_lines: Vec<usize>,
    pub errors: Vec<String>,
    pub sources: Vec<PathBuf>,
}
impl Expansion {
    pub fn html(&self, syntaxes: Option<&yy_syntax::Registry>) -> String {
        if self.markdown.is_empty() && !self.errors.is_empty() {
            return format!(
                "<blockquote><strong>埋め込みエラー:</strong><pre>{}</pre></blockquote>",
                crate::escape(&self.errors.join("\n"))
            );
        }
        crate::markdown_with_source_lines(&self.markdown, syntaxes, &self.source_lines)
    }
}
/// Read only the note's folder tree and the snippets tree. Export rebases local links.
pub fn expand(
    source: &str,
    document: Option<&Path>,
    snippets: Option<&Path>,
    export: bool,
) -> Expansion {
    let root = document
        .and_then(Path::parent)
        .and_then(|p| p.canonicalize().ok());
    let snippets = snippets.and_then(|p| p.canonicalize().ok());
    let current = document.and_then(|p| {
        p.canonicalize()
            .ok()
            .or_else(|| Some(p.parent()?.canonicalize().ok()?.join(p.file_name()?)))
    });
    let mut context = Context {
        root,
        snippets,
        export,
        files: BTreeMap::new(),
        stack: Vec::new(),
        errors: Vec::new(),
        embeds: 0,
        bytes: 0,
        entries: 0,
    };
    if let Some(path) = &current {
        context.files.insert(path.clone(), source.to_owned());
    }
    let (markdown, source_lines) = context.render(source, current.as_deref(), None, 0);
    Expansion {
        markdown,
        source_lines,
        errors: context.errors,
        sources: context.files.into_keys().collect(),
    }
}
fn options() -> Options {
    crate::markdown::options()
}
fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains(['\\', ':', '\0', '%'])
        && !name.chars().any(char::is_control)
        && Path::new(name)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
}
struct Context {
    root: Option<PathBuf>,
    snippets: Option<PathBuf>,
    export: bool,
    files: BTreeMap<PathBuf, String>,
    stack: Vec<(PathBuf, String)>,
    errors: Vec<String>,
    embeds: usize,
    bytes: usize,
    entries: usize,
}
impl Context {
    fn allowed(&self, path: &Path) -> bool {
        self.root
            .iter()
            .chain(self.snippets.iter())
            .any(|r| path.starts_with(r))
    }
    fn resolve(&mut self, name: &str, current: Option<&Path>) -> Result<PathBuf, String> {
        if name.is_empty() {
            return current
                .map(Path::to_path_buf)
                .ok_or("同じノートの参照には保存先が必要です".into());
        }
        if !safe_name(name) {
            return Err("参照名にはフォルダ内の相対パスを指定してください".into());
        }
        let mut relative = PathBuf::from(name);
        if relative.extension().is_none() {
            relative.set_extension("md");
        }
        if crate::Kind::detect(None, Some(&relative)) != Some(crate::Kind::Markdown) {
            return Err("埋め込みはMarkdownファイルに限定されています".into());
        }
        let local = current.and_then(Path::parent).or(self.root.as_deref());
        for base in local
            .into_iter()
            .chain(self.root.as_deref())
            .chain(self.snippets.as_deref())
        {
            if let Ok(path) = base.join(&relative).canonicalize() {
                if !self.allowed(&path) {
                    return Err("許可されたフォルダ外への参照です".into());
                }
                if path.is_file() {
                    return Ok(path);
                }
            }
        }
        if relative.components().count() > 1 {
            return Err(format!("ファイルが見つかりません: {name}"));
        }
        let mut found = BTreeSet::new();
        for root in self.root.clone().into_iter().chain(self.snippets.clone()) {
            let mut dirs = vec![(root, 0usize)];
            let mut visited = BTreeSet::new();
            while let Some((dir, depth)) = dirs.pop() {
                let dir = dir.canonicalize().map_err(|e| e.to_string())?;
                if !self.allowed(&dir) || !visited.insert(dir.clone()) {
                    continue;
                }
                for entry in
                    std::fs::read_dir(&dir).map_err(|e| format!("フォルダを探索できません: {e}"))?
                {
                    self.entries += 1;
                    if self.entries > 20000 {
                        return Err(
                            "参照探索が20000項目を超えました。フォルダを指定してください".into(),
                        );
                    }
                    let entry = entry.map_err(|e| e.to_string())?;
                    let kind = entry.file_type().map_err(|e| e.to_string())?;
                    if kind.is_symlink() {
                        continue;
                    }
                    if kind.is_dir() && depth < 32 {
                        dirs.push((entry.path(), depth + 1));
                    }
                    if kind.is_file()
                        && entry
                            .file_name()
                            .to_string_lossy()
                            .eq_ignore_ascii_case(&relative.to_string_lossy())
                    {
                        let path = entry.path().canonicalize().map_err(|e| e.to_string())?;
                        if self.allowed(&path) {
                            found.insert(path);
                        }
                    }
                }
            }
        }
        if found.len() > 1 {
            return Err(format!(
                "同名ファイルが複数あります。フォルダを指定してください: {name}"
            ));
        }
        found
            .into_iter()
            .next()
            .ok_or_else(|| format!("ファイルが見つかりません: {name}"))
    }
    fn read(&mut self, path: &Path) -> Result<String, String> {
        if let Some(text) = self.files.get(path) {
            return Ok(text.clone());
        }
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .map_err(|e| e.to_string())?
            .take(MAX_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        self.bytes = self.bytes.saturating_add(bytes.len());
        if self.bytes > MAX_BYTES {
            return Err("埋め込み元の合計が8 MiBを超えました".into());
        }
        let text =
            String::from_utf8(bytes).map_err(|_| "参照先はUTF-8のMarkdownで保存してください")?;
        let text = text.trim_start_matches('\u{feff}').to_owned();
        self.files.insert(path.to_owned(), text.clone());
        Ok(text)
    }
    fn embed(
        &mut self,
        target: &str,
        current: Option<&Path>,
        line: usize,
        depth: usize,
    ) -> Result<String, String> {
        self.embeds += 1;
        if self.embeds > MAX_EMBEDS || depth >= 16 {
            return Err("埋め込みは深さ16・合計256個までです".into());
        }
        let (name, fragment) = target.split_once('#').unwrap_or((target, ""));
        let path = self.resolve(name.trim(), current)?;
        let key = (path.clone(), fragment.to_owned());
        if self.stack.contains(&key) {
            return Err(format!("循環する埋め込みです: {target}"));
        }
        let text = self.read(&path)?;
        let selected = select(&text, fragment)?;
        self.stack.push(key);
        let (result, _) = self.render(&selected, Some(&path), Some(line), depth + 1);
        self.stack.pop();
        Ok(result)
    }
    fn render(
        &mut self,
        source: &str,
        current: Option<&Path>,
        origin: Option<usize>,
        depth: usize,
    ) -> (String, Vec<usize>) {
        let mut replacements = Vec::new();
        let mut replacement_bytes = 0usize;
        for (event, range) in Parser::new_ext(source, options()).into_offset_iter() {
            if let Event::Start(Tag::Image {
                link_type: LinkType::WikiLink { .. },
                dest_url,
                ..
            }) = &event
            {
                let line = origin.unwrap_or_else(|| {
                    source[..range.start]
                        .bytes()
                        .filter(|b| *b == b'\n')
                        .count()
                });
                let value = match self.embed(dest_url, current, line, depth) {
                    Ok(value) => value,
                    Err(error) => {
                        let error = format!("{dest_url}: {error}");
                        if self.errors.len() < MAX_EMBEDS {
                            self.errors.push(error.clone());
                        }
                        format!(
                            "\n\n> **埋め込みエラー:** {}\n\n",
                            crate::escape(&error.replace(['\n', '\r'], " "))
                        )
                    }
                };
                let prefix =
                    &source[source[..range.start].rfind('\n').map_or(0, |n| n + 1)..range.start];
                let value = contextual_embed(&value, prefix);
                replacement_bytes = replacement_bytes.saturating_add(value.len());
                if replacement_bytes > MAX_BYTES {
                    self.errors.push("展開結果が8 MiBを超えました".into());
                    return (String::new(), vec![0]);
                }
                replacements.push((range, value, line));
            } else if let Event::Start(
                Tag::Link {
                    link_type,
                    dest_url,
                    title,
                    ..
                }
                | Tag::Image {
                    link_type,
                    dest_url,
                    title,
                    ..
                },
            ) = &event
            {
                if matches!(link_type, LinkType::WikiLink { .. }) {
                    continue;
                }
                if let Some(url) = self.rebase_url(dest_url, current) {
                    let raw = &source[range.clone()];
                    if let Some(end) = label_end(raw) {
                        let title = if title.is_empty() {
                            String::new()
                        } else {
                            format!(" \"{}\"", title.replace('"', "&quot;"))
                        };
                        let value = format!("{}(<{}>{title})", &raw[..=end], url);
                        replacement_bytes = replacement_bytes.saturating_add(value.len());
                        if replacement_bytes > MAX_BYTES {
                            self.errors.push("展開結果が8 MiBを超えました".into());
                            return (String::new(), vec![0]);
                        }
                        let line = origin.unwrap_or_else(|| {
                            source[..range.start]
                                .bytes()
                                .filter(|b| *b == b'\n')
                                .count()
                        });
                        replacements.push((range, value, line));
                    }
                }
            }
        }
        replacements.sort_by_key(|(range, _, _)| range.start);
        let mut output = String::new();
        let mut lines = Vec::new();
        let mut cursor = 0;
        let mut line = origin.unwrap_or(0);
        for (range, value, at) in replacements {
            if range.start < cursor {
                continue;
            }
            append(
                &mut output,
                &mut lines,
                &source[cursor..range.start],
                &mut line,
                origin.is_none(),
            );
            if output.len().saturating_add(value.len()) > MAX_BYTES {
                self.errors.push("展開結果が8 MiBを超えました".into());
                return (String::new(), vec![0]);
            }
            let mut fixed = at;
            append(&mut output, &mut lines, &value, &mut fixed, false);
            if origin.is_none() {
                line += source[range.clone()]
                    .bytes()
                    .filter(|b| *b == b'\n')
                    .count();
            }
            cursor = range.end;
        }
        if output.len().saturating_add(source.len() - cursor) > MAX_BYTES {
            self.errors.push("展開結果が8 MiBを超えました".into());
            return (String::new(), vec![0]);
        }
        append(
            &mut output,
            &mut lines,
            &source[cursor..],
            &mut line,
            origin.is_none(),
        );
        (output, lines)
    }
    fn rebase_url(&self, url: &str, current: Option<&Path>) -> Option<String> {
        if url.starts_with(['#', '/']) || url.contains(':') {
            return None;
        }
        let (path, suffix) = url
            .find(['#', '?'])
            .map_or((url, ""), |n| (&url[..n], &url[n..]));
        let relative = path
            .split('/')
            .map(|part| {
                if matches!(part, "." | "..") {
                    Some(part.to_owned())
                } else {
                    crate::security::document_relative_path(part)
                }
            })
            .collect::<Option<Vec<_>>>()?
            .join("/");
        let base = current.and_then(Path::parent).or(self.root.as_deref())?;
        let resolved = base.join(relative).canonicalize().ok()?;
        if !self.allowed(&resolved) {
            return None;
        }
        let target = if self.export {
            file_url(&resolved)
        } else if let Some(rel) = self
            .root
            .as_deref()
            .and_then(|r| resolved.strip_prefix(r).ok())
        {
            encode_path(&rel.to_string_lossy().replace('\\', "/"))
        } else {
            let rel = resolved.strip_prefix(self.snippets.as_deref()?).ok()?;
            format!(
                "{SNIPPET_PREFIX}{}",
                encode_path(&rel.to_string_lossy().replace('\\', "/"))
            )
        };
        Some(target + suffix)
    }
}
fn append(out: &mut String, lines: &mut Vec<usize>, text: &str, line: &mut usize, advance: bool) {
    for chunk in text.split_inclusive('\n') {
        if out.is_empty() || out.ends_with('\n') {
            lines.push(*line);
        }
        out.push_str(chunk);
        if advance && chunk.ends_with('\n') {
            *line += 1;
        }
    }
}
fn contextual_embed(value: &str, prefix: &str) -> String {
    let trimmed = prefix.trim_start_matches([' ', '\t', '>']);
    let list = trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
        .or_else(|| trimmed.strip_prefix("+ "));
    let numbered = trimmed
        .find(['.', ')'])
        .filter(|&at| at > 0 && trimmed[..at].bytes().all(|b| b.is_ascii_digit()))
        .and_then(|at| trimmed[at + 1..].strip_prefix(' '));
    if trimmed.is_empty() || list == Some("") || numbered == Some("") {
        let continuation: String = prefix
            .chars()
            .map(|c| if matches!(c, '>' | '\t') { c } else { ' ' })
            .collect();
        let value = value.trim_end().replace("\r\n", "\n");
        value
            .split('\n')
            .collect::<Vec<_>>()
            .join(&format!("\n{continuation}"))
    } else {
        format!("\n\n{}\n\n", value.trim_end())
    }
}
fn label_end(raw: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut escaped = false;
    for (at, ch) in raw.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == '[' {
            depth += 1;
        }
        if ch == ']' {
            depth = depth.checked_sub(1)?;
            if depth == 0 {
                return Some(at);
            }
        }
    }
    None
}
fn encode_path(path: &str) -> String {
    let mut out = String::new();
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'_' | b'.' | b'~' | b':') {
            out.push(byte as char);
        } else {
            use std::fmt::Write;
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}
fn file_url(path: &Path) -> String {
    let raw = path.to_string_lossy().replace('\\', "/");
    let raw = raw
        .strip_prefix("//?/UNC/")
        .map(|p| format!("//{p}"))
        .unwrap_or_else(|| raw.strip_prefix("//?/").unwrap_or(&raw).to_owned());
    if raw.starts_with("//") {
        format!("file:{}", encode_path(&raw))
    } else if raw.starts_with('/') {
        format!("file://{}", encode_path(&raw))
    } else {
        format!("file:///{}", encode_path(&raw))
    }
}
fn select(source: &str, fragment: &str) -> Result<String, String> {
    if fragment.is_empty() {
        return Ok(source.to_owned());
    }
    if let Some(id) = fragment.strip_prefix('^') {
        return block(source, id);
    }
    let mut headings = Vec::new();
    let mut current = None;
    for (event, range) in Parser::new_ext(source, options()).into_offset_iter() {
        match event {
            Event::Start(Tag::Heading { level, id, .. }) => {
                current = Some((range, level as u8, id.map(|i| i.to_string()), String::new()))
            }
            Event::Text(text) | Event::Code(text) => {
                if let Some(h) = &mut current {
                    h.3.push_str(&text);
                }
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some(h) = current.take() {
                    headings.push(h);
                }
            }
            _ => {}
        }
    }
    let matches: Vec<_> = headings
        .iter()
        .enumerate()
        .filter(|(_, h)| h.3 == fragment || h.2.as_deref() == Some(fragment))
        .collect();
    if matches.len() != 1 {
        return Err(if matches.is_empty() {
            format!("見出しが見つかりません: {fragment}")
        } else {
            format!("同じ見出しが複数あります: {fragment}")
        });
    }
    let (index, heading) = matches[0];
    let end = headings
        .iter()
        .skip(index + 1)
        .find(|h| h.1 <= heading.1)
        .map_or(source.len(), |h| h.0.start);
    Ok(source[heading.0.start..end].to_owned())
}
fn block(source: &str, id: &str) -> Result<String, String> {
    if id.is_empty() || !id.chars().all(|c| c.is_alphanumeric() || c == '-') {
        return Err("ブロックIDは英数字・ハイフンで指定してください".into());
    }
    let mut candidates: Vec<Range<usize>> = Vec::new();
    let mut items: Vec<Range<usize>> = Vec::new();
    let mut excluded = Vec::new();
    for (event, range) in Parser::new_ext(source, options()).into_offset_iter() {
        match event {
            Event::Start(Tag::CodeBlock(_) | Tag::HtmlBlock | Tag::MetadataBlock(_))
            | Event::Code(_) => excluded.push(range),
            Event::Start(Tag::Item) => {
                items.push(range.clone());
                candidates.push(range);
            }
            Event::Start(Tag::Paragraph | Tag::List(_) | Tag::BlockQuote(_) | Tag::Table(_)) => {
                candidates.push(range)
            }
            _ => {}
        }
    }
    let marker = format!("^{id}");
    let mut found = Vec::new();
    let mut offset = 0;
    for line in source.split_inclusive('\n') {
        let trimmed = line.trim_end();
        if let Some(prefix) = trimmed.strip_suffix(&marker) {
            let at = offset + prefix.len();
            if (prefix.is_empty() || prefix.ends_with(char::is_whitespace))
                && !excluded.iter().any(|r| r.contains(&at))
            {
                let range = if prefix.trim().is_empty() {
                    candidates
                        .iter()
                        .filter(|r| r.end <= offset)
                        .max_by_key(|r| (r.end, r.len()))
                        .cloned()
                } else {
                    items
                        .iter()
                        .filter(|r| r.contains(&at))
                        .min_by_key(|r| r.len())
                        .cloned()
                        .or_else(|| {
                            candidates
                                .iter()
                                .filter(|r| r.contains(&at))
                                .min_by_key(|r| r.len())
                                .cloned()
                        })
                };
                if let Some(range) = range {
                    found.push((range, at..at + marker.len()));
                }
            }
        }
        offset += line.len();
    }
    if found.len() != 1 {
        return Err(format!("ブロックが見つからないかIDが重複しています: {id}"));
    }
    let (range, marker) = found.pop().unwrap();
    let mut text = source[range.clone()].to_owned();
    if range.contains(&marker.start) {
        text.replace_range(marker.start - range.start..marker.end - range.start, "");
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn put(root: &Path, name: &str, value: &str) -> PathBuf {
        let path = root.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, value).unwrap();
        path
    }
    #[test]
    fn whole_section_block_and_nested_snippets() {
        let dir = tempfile::tempdir().unwrap();
        let snippets = tempfile::tempdir().unwrap();
        let main = put(dir.path(), "main.md", "");
        put(
            dir.path(),
            "parts/要件定義.md",
            "# 要件\n全体\n\n## 非機能要件\n性能\n\n### 詳細\n応答時間\n\n## 機能要件\n対象外\n\n指定段落\n2行目 ^important\n\n他の段落\n",
        );
        put(snippets.path(), "共通ヘッダー.md", "# 共通\n![[署名]]\n");
        put(snippets.path(), "署名.md", "署名です\n");
        let source = "# 本文\n\n![[共通ヘッダー]]\n\n![[要件定義#非機能要件]]\n\n![[要件定義#^important]]\n\n終了\n";
        let out = expand(source, Some(&main), Some(snippets.path()), false);
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        assert!(out.markdown.contains("署名です") && out.markdown.contains("### 詳細"));
        assert!(out.markdown.contains("指定段落\n2行目") && !out.markdown.contains("^important"));
        assert!(!out.markdown.contains("対象外") && !out.markdown.contains("他の段落"));
        let html = out.html(None);
        assert!(html.contains("<h2") && html.contains("data-line=\"4\""));
        assert!(!html.contains("![[") && html.contains("終了"));
    }
    #[test]
    fn code_escaping_missing_and_duplicate_files() {
        let dir = tempfile::tempdir().unwrap();
        let main = put(dir.path(), "main.md", "");
        put(dir.path(), "a/shared.md", "A");
        put(dir.path(), "b/shared.md", "B");
        let literal =
            "`![[missing]]`\n\n```md\n![[missing]]\n```\n\n\\![[missing]]\n\n    ![[missing]]\n";
        let out = expand(literal, Some(&main), None, false);
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        assert_eq!(out.markdown, literal);
        let out = expand(
            "![[shared]]\n![[a/shared]]\n![[absent]]",
            Some(&main),
            None,
            false,
        );
        assert_eq!(out.errors.len(), 2);
        assert!(out.markdown.contains("A"));
        assert!(out.errors[0].contains("同名"));
    }
    #[test]
    fn cycles_same_note_and_unselected_cycles() {
        let dir = tempfile::tempdir().unwrap();
        let main = put(
            dir.path(),
            "main.md",
            "# 共通\n再利用\n\n# 本文\n![[#共通]]\n",
        );
        let source = std::fs::read_to_string(&main).unwrap();
        assert!(expand(&source, Some(&main), None, false).errors.is_empty());
        put(dir.path(), "a.md", "![[b]]");
        put(dir.path(), "b.md", "![[a]]");
        let out = expand("![[a]]", Some(&main), None, false);
        assert!(out.errors.iter().any(|e| e.contains("循環")));
    }
    #[test]
    fn traversal_external_paths_and_non_markdown_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let main = put(dir.path(), "main.md", "");
        for target in [
            "../secret",
            "C:/secret",
            "https://example.com/note",
            "/secret",
            "%2e%2e/secret",
            "file.txt",
            "a\\secret",
        ] {
            let out = expand(&format!("![[{target}]]"), Some(&main), None, false);
            assert!(!out.errors.is_empty(), "{target}");
        }
    }
    #[test]
    fn links_images_and_export_rebase_and_line_mapping() {
        let dir = tempfile::tempdir().unwrap();
        let snippets = tempfile::tempdir().unwrap();
        let main = put(dir.path(), "main.md", "");
        put(dir.path(), "parts/image 日本.png", "png");
        put(
            dir.path(),
            "parts/note.md",
            "![画像](<image 日本.png>)\n\n[リンク](other.md#見出し)\n",
        );
        put(dir.path(), "parts/other.md", "# 見出し");
        put(snippets.path(), "icon.png", "png");
        put(snippets.path(), "snippet.md", "![icon](icon.png)");
        let out = expand(
            "前\n\n![[parts/note]]\n\n![[snippet]]\n\n後\n",
            Some(&main),
            Some(snippets.path()),
            false,
        );
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        assert!(
            out.markdown
                .contains("parts/image%20%E6%97%A5%E6%9C%AC.png")
        );
        assert!(out.markdown.contains("parts/other.md#見出し"));
        assert!(out.markdown.contains("__yy_snippets__/icon.png"));
        let after = out.markdown.lines().position(|s| s == "後").unwrap();
        assert_eq!(out.source_lines[after], 6);
        let export = expand("![[parts/note]]", Some(&main), None, true);
        assert!(export.markdown.contains("file:///"));
        assert!(!export.markdown.contains("__yy_snippets__"));
    }
    #[test]
    fn block_lists_tables_and_missing_sections() {
        assert_eq!(
            select("- 一つ目\n- 二つ目\n\n^list\n", "^list")
                .unwrap()
                .trim(),
            "- 一つ目\n- 二つ目"
        );
        assert!(
            select("- 対象 ^item\n- 他\n", "^item")
                .unwrap()
                .contains("対象")
        );
        let table = "| A | B |\n|---|---|\n| 1 | 2 |\n\n^table\n";
        assert!(select(table, "^table").unwrap().contains("| 1 | 2 |"));
        assert!(select("```\nignored ^code\n```", "^code").is_err());
        assert!(select("# A\n# A", "A").is_err());
        assert!(select("# A", "B").is_err());
        assert!(
            select("Setext\n======\nBody\n\nNext\n----\nOther", "Setext")
                .unwrap()
                .contains("Body")
        );
    }
    #[test]
    fn count_and_byte_limits_fail_visibly() {
        let dir = tempfile::tempdir().unwrap();
        let main = put(dir.path(), "main.md", "");
        put(dir.path(), "small.md", "small");
        let out = expand(&"![[small]]\n".repeat(257), Some(&main), None, false);
        assert!(out.errors.iter().any(|e| e.contains("256")));
        put(dir.path(), "large.md", &"x".repeat(MAX_BYTES + 1));
        assert!(
            !expand("![[large]]", Some(&main), None, false)
                .errors
                .is_empty()
        );
    }
    #[test]
    fn containers_aliases_unsaved_notes_and_parent_asset_paths() {
        let oversized = expand(&"x".repeat(MAX_BYTES + 1), None, None, false);
        assert!(oversized.html(None).contains("8 MiB"));
        let dir = tempfile::tempdir().unwrap();
        let large_note = put(dir.path(), "large-root.md", "");
        put(dir.path(), "medium.md", &"x".repeat(MAX_BYTES / 2));
        let large_output = expand(&"![[medium]]\n".repeat(256), Some(&large_note), None, false);
        assert!(large_output.html(None).contains("8 MiB"));
        let main = put(dir.path(), "main.md", "");
        put(dir.path(), "part.md", "段落一\n\n段落二\n");
        let out = expand(
            "> ![[part|別名]]\n\n- ![[part]]\n",
            Some(&main),
            None,
            false,
        );
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        let html = out.html(None);
        assert!(html.contains("<blockquote>") && html.contains("<li>"));
        assert_eq!(html.matches("段落二").count(), 2);
        let unsaved = expand("![[part]]", None, Some(dir.path()), false);
        assert!(unsaved.errors.is_empty() && unsaved.markdown.contains("段落一"));
        put(dir.path(), "asset.png", "image");
        put(dir.path(), "child/asset.md", "![親の画像](../asset.png)");
        let out = expand("![[child/asset]]", Some(&main), None, false);
        assert!(out.markdown.contains("(<asset.png>)"), "{}", out.markdown);
        for n in 0..18 {
            put(
                dir.path(),
                &format!("depth{n}.md"),
                &format!("![[depth{}]]", n + 1),
            );
        }
        assert!(
            expand("![[depth0]]", Some(&main), None, false)
                .errors
                .iter()
                .any(|e| e.contains("深さ16"))
        );
    }
    #[cfg(unix)]
    #[test]
    fn symlink_escape_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let main = put(dir.path(), "main.md", "");
        put(outside.path(), "secret.md", "secret");
        std::os::unix::fs::symlink(
            outside.path().join("secret.md"),
            dir.path().join("escape.md"),
        )
        .unwrap();
        assert!(
            !expand("![[escape]]", Some(&main), None, false)
                .errors
                .is_empty()
        );
    }
}
