//! Markdown を HTML にする。
//!
//! CommonMark に加えて拡張構文（表・脚注・取り消し線・タスクリスト・定義リスト・上付き/下付き・
//! 見出しの属性・GitHub のアラート `> [!NOTE]`・Wiki リンク・YAML のフロントマター）を扱う。
//!
//! * ```` ```mermaid ```` のコードブロックは mermaid.js が描く `<pre class="mermaid">` にする
//! * `$…$`・`$$…$$` と ```` ```math ```` は KaTeX が描く `.math` の要素にする
//! * その他のコードブロックは yy-syntax の定義で色付けする（`tok-*` のクラス）
//! * 見出しには GitHub と同じ規則の `id` を付ける（ページ内リンク用）
//! * 最上位のブロックの先頭に元の行番号（`data-line`）を付ける（エディタとのスクロール同期用）

use std::collections::HashMap;

use pulldown_cmark::{
    CodeBlockKind, Event, HeadingLevel, MetadataBlockKind, Options, Parser, Tag, TagEnd,
};

/// 変換に使う拡張構文。
fn options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_HEADING_ATTRIBUTES
        | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS
        | Options::ENABLE_MATH
        | Options::ENABLE_GFM
        | Options::ENABLE_DEFINITION_LIST
        | Options::ENABLE_SUPERSCRIPT
        | Options::ENABLE_SUBSCRIPT
        | Options::ENABLE_WIKILINKS
}

/// Markdown を HTML（`<body>` の中身）にする。`syntaxes` があればコードブロックを色付けする。
pub fn markdown_to_html(src: &str, syntaxes: Option<&yy_syntax::Registry>) -> String {
    let lines = LineIndex::new(src);
    let mut out_events: Vec<Event> = Vec::new();
    let mut slugs = Slugs::default();
    let mut depth = 0usize;
    // 脚注の定義（末尾に置く）と、処理中の定義の開始位置
    let mut footnotes: Vec<Event> = Vec::new();
    let mut footnote_start = None;
    let mut iter = Parser::new_ext(src, options()).into_offset_iter();
    while let Some((ev, range)) = iter.next() {
        match ev {
            Event::Start(Tag::Heading {
                level,
                id,
                classes,
                attrs,
            }) => {
                // 見出しの中身を集めて id を決める
                let mut inner = Vec::new();
                let mut text = String::new();
                for (e, _) in iter.by_ref() {
                    if matches!(e, Event::End(TagEnd::Heading(_))) {
                        break;
                    }
                    if let Event::Text(t) | Event::Code(t) = &e {
                        text.push_str(t);
                    }
                    inner.push(e);
                }
                let id = match id {
                    Some(id) => slugs.reserve(&id),
                    None => slugs.make(&text),
                };
                let mut open = format!("<{} id=\"{}\"", level_tag(level), escape(&id));
                if !classes.is_empty() {
                    let c: Vec<&str> = classes.iter().map(|c| c.as_ref()).collect();
                    open += &format!(" class=\"{}\"", escape(&c.join(" ")));
                }
                for (k, v) in &attrs {
                    open += &format!(" {}", escape(k));
                    if let Some(v) = v {
                        open += &format!("=\"{}\"", escape(v));
                    }
                }
                if depth == 0 {
                    open += &format!(" data-line=\"{}\"", lines.line_of(range.start));
                }
                open.push('>');
                out_events.push(Event::Html(open.into()));
                out_events.extend(inner);
                out_events.push(Event::Html(format!("</{}>\n", level_tag(level)).into()));
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                let mut code = String::new();
                for (e, _) in iter.by_ref() {
                    match e {
                        Event::End(TagEnd::CodeBlock) => break,
                        Event::Text(t) => code.push_str(&t),
                        _ => {}
                    }
                }
                let lang = match &kind {
                    CodeBlockKind::Fenced(info) => info
                        .split(|c: char| c.is_whitespace() || c == ',' || c == '{')
                        .next()
                        .unwrap_or("")
                        .to_owned(),
                    CodeBlockKind::Indented => String::new(),
                };
                let line = (depth == 0).then(|| lines.line_of(range.start));
                out_events.push(Event::Html(code_block(&lang, &code, line, syntaxes).into()));
            }
            Event::Start(Tag::MetadataBlock(kind)) => {
                let mut text = String::new();
                for (e, _) in iter.by_ref() {
                    match e {
                        Event::End(TagEnd::MetadataBlock(_)) => break,
                        Event::Text(t) => text.push_str(&t),
                        _ => {}
                    }
                }
                let lang = match kind {
                    MetadataBlockKind::YamlStyle => "yaml",
                    MetadataBlockKind::PlusesStyle => "toml",
                };
                let mut html = code_block(lang, &text, Some(lines.line_of(range.start)), syntaxes);
                html = html.replacen("<pre", "<pre class=\"front-matter\"", 1);
                out_events.push(Event::Html(html.into()));
            }
            Event::Start(tag @ Tag::FootnoteDefinition(_)) if depth == 0 => {
                // 脚注の定義は文書の末尾にまとめる（End で取り出す）
                footnote_start = Some(out_events.len());
                depth += 1;
                out_events.push(Event::Start(tag));
            }
            Event::Start(tag) => {
                if depth == 0 {
                    // 最上位のブロックの前に行番号の目印を置く
                    out_events.push(Event::Html(
                        format!(
                            "<span class=\"yy-line\" data-line=\"{}\"></span>",
                            lines.line_of(range.start)
                        )
                        .into(),
                    ));
                }
                depth += 1;
                out_events.push(Event::Start(tag));
            }
            Event::End(tag) => {
                depth = depth.saturating_sub(1);
                out_events.push(Event::End(tag));
                if depth == 0
                    && let Some(start) = footnote_start.take()
                {
                    footnotes.extend(out_events.drain(start..));
                }
            }
            Event::Rule if depth == 0 => {
                out_events.push(Event::Html(
                    format!(
                        "<span class=\"yy-line\" data-line=\"{}\"></span>",
                        lines.line_of(range.start)
                    )
                    .into(),
                ));
                out_events.push(Event::Rule);
            }
            e => out_events.push(e),
        }
    }
    join_cjk_lines(&mut out_events);
    if !footnotes.is_empty() {
        out_events.push(Event::Html("<section class=\"footnotes\">\n".into()));
        out_events.extend(footnotes);
        out_events.push(Event::Html("</section>\n".into()));
    }
    let mut html = String::with_capacity(src.len() * 3 / 2);
    pulldown_cmark::html::push_html(&mut html, out_events.into_iter());
    html
}

/// 日本語などの文の途中の改行（ソフト改行）を消す。そのままではブラウザが空白として表示し、
/// 「です。 次の文」のように余計な空きができる。
fn join_cjk_lines(events: &mut Vec<Event>) {
    let is_cjk = |c: char| {
        matches!(c,
            '\u{2E80}'..='\u{9FFF}'      // 部首・記号・かな・漢字
            | '\u{F900}'..='\u{FAFF}'    // 互換漢字
            | '\u{FF00}'..='\u{FFEF}'    // 全角英数・半角カナ
            | '\u{20000}'..='\u{3FFFF}') // 拡張漢字
    };
    let text_edge = |e: &Event, last: bool| match e {
        Event::Text(t) | Event::Code(t) => {
            if last {
                t.chars().next_back()
            } else {
                t.chars().next()
            }
        }
        _ => None,
    };
    let mut i = 1;
    while i + 1 < events.len() {
        if matches!(events[i], Event::SoftBreak)
            && text_edge(&events[i - 1], true).is_some_and(is_cjk)
            && text_edge(&events[i + 1], false).is_some_and(is_cjk)
        {
            events.remove(i);
        } else {
            i += 1;
        }
    }
}

fn level_tag(level: HeadingLevel) -> &'static str {
    match level {
        HeadingLevel::H1 => "h1",
        HeadingLevel::H2 => "h2",
        HeadingLevel::H3 => "h3",
        HeadingLevel::H4 => "h4",
        HeadingLevel::H5 => "h5",
        HeadingLevel::H6 => "h6",
    }
}

/// コードブロックの HTML。`line` は最上位のブロックなら元の行番号。
fn code_block(
    lang: &str,
    code: &str,
    line: Option<usize>,
    syntaxes: Option<&yy_syntax::Registry>,
) -> String {
    let data_line = line.map_or(String::new(), |l| format!(" data-line=\"{l}\""));
    match lang.to_ascii_lowercase().as_str() {
        "mermaid" => format!("<pre class=\"mermaid\"{data_line}>{}</pre>\n", escape(code)),
        "math" | "latex" | "katex" => format!(
            "<div class=\"math math-display\"{data_line}>{}</div>\n",
            escape(code)
        ),
        _ => {
            let body = syntaxes
                .filter(|_| !lang.is_empty())
                .and_then(|r| r.get(lang).ok())
                .map(|s| highlight(&s, code))
                .unwrap_or_else(|| escape(code));
            let class = if lang.is_empty() {
                String::new()
            } else {
                format!(" class=\"language-{}\"", escape(lang))
            };
            format!("<pre{data_line}><code{class}>{body}</code></pre>\n")
        }
    }
}

/// コードを yy-syntax の定義で色付けした HTML（`<span class="tok-…">`）。
fn highlight(syntax: &yy_syntax::Syntax, code: &str) -> String {
    let mut out = String::with_capacity(code.len() * 2);
    let mut state = yy_syntax::LineState::default();
    let mut spans = Vec::new();
    for line in code.split_inclusive('\n') {
        let content = line.trim_end_matches('\n').trim_end_matches('\r');
        spans.clear();
        state = syntax.highlight_line(&state, content.as_bytes(), &mut spans);
        let mut pos = 0usize;
        for sp in &spans {
            let (a, b) = (sp.range.start as usize, sp.range.end as usize);
            // 範囲が文字の途中にかかっていたら色を付けない
            if a < pos || !content.is_char_boundary(a) || !content.is_char_boundary(b) {
                continue;
            }
            out += &escape(&content[pos..a]);
            out += &format!(
                "<span class=\"{}\">{}</span>",
                token_classes(syntax.token_name(sp.token)),
                escape(&content[a..b])
            );
            pos = b;
        }
        out += &escape(&content[pos..]);
        out += &escape(&line[content.len()..]);
    }
    out
}

/// トークン名（`keyword.control`）の CSS クラス（`tok-keyword tok-keyword-control`）。
pub fn token_classes(name: &str) -> String {
    let mut classes = Vec::new();
    let mut prefix = String::new();
    for part in name.split('.') {
        if !prefix.is_empty() {
            prefix.push('-');
        }
        prefix.push_str(&css_ident(part));
        classes.push(format!("tok-{prefix}"));
    }
    classes.join(" ")
}

/// CSS の識別子に使える文字だけにする。
pub(crate) fn css_ident(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// HTML の特殊文字をエスケープする。
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// 見出しの id（GitHub と同じ規則: 小文字にし、英数字・`-`・`_`・英字以外の文字（日本語など）以外を
/// 除き、空白を `-` にする。重複には `-1`・`-2` を付ける）。
#[derive(Default)]
struct Slugs {
    used: HashMap<String, usize>,
}

impl Slugs {
    fn make(&mut self, text: &str) -> String {
        let base: String = text
            .trim()
            .to_lowercase()
            .chars()
            .filter_map(|c| {
                if c == ' ' {
                    Some('-')
                } else if c.is_alphanumeric() || c == '-' || c == '_' {
                    Some(c)
                } else {
                    None
                }
            })
            .collect();
        self.reserve(&base)
    }

    fn reserve(&mut self, base: &str) -> String {
        let Some(&count) = self.used.get(base) else {
            self.used.insert(base.to_owned(), 0);
            return base.to_owned();
        };
        let mut n = count;
        loop {
            n += 1;
            let candidate = format!("{base}-{n}");
            if !self.used.contains_key(&candidate) {
                self.used.insert(base.to_owned(), n);
                self.used.insert(candidate.clone(), 0);
                return candidate;
            }
        }
    }
}

/// バイト位置から行番号（0 始まり）を求める。
struct LineIndex {
    starts: Vec<usize>,
}

impl LineIndex {
    fn new(src: &str) -> LineIndex {
        let mut starts = vec![0];
        starts.extend(memchr_newlines(src.as_bytes()));
        LineIndex { starts }
    }

    fn line_of(&self, offset: usize) -> usize {
        self.starts.partition_point(|&s| s <= offset) - 1
    }
}

fn memchr_newlines(b: &[u8]) -> impl Iterator<Item = usize> + '_ {
    b.iter()
        .enumerate()
        .filter(|&(_, &c)| c == b'\n')
        .map(|(i, _)| i + 1)
}
