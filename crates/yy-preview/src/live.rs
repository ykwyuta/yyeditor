//! Markdown ライブ編集の表示と、原文を変更せずに扱うための位置変換。
use crate::{DOC_HOST, PREVIEW_HOST, escape, json_string};
use pulldown_cmark::{Event, Options, Parser, Tag, html};
use std::ops::Range;
pub mod table;

pub const MAX_BYTES: usize = 1 << 20;

pub fn source_json(source: &str) -> String {
    json_string(source)
}

/// 最上位ブロックごとの表示。境界は JavaScript の UTF-16 オフセット。
/// HTML は文字として表示し、文書内のスクリプトを編集ページでは実行しない。
pub fn blocks(source: &str) -> String {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES;
    let mut out = String::new();
    let mut events = Vec::new();
    let mut depth = 0;
    let mut start = 0;
    let mut end = 0;
    let mut utf16 = 0;
    let mut cells = Vec::new();
    for (event, range) in Parser::new_ext(source, options).into_offset_iter() {
        if matches!(event, Event::Start(Tag::TableCell)) {
            cells.push(source[..range.start].encode_utf16().count());
        }
        end = end.max(range.end);
        match &event {
            Event::Start(_) => depth += 1,
            Event::End(_) => depth -= 1,
            _ => {}
        }
        events.push(match event {
            Event::Html(text) | Event::InlineHtml(text) => Event::Text(text),
            other => other,
        });
        if depth == 0 {
            let mut body = String::new();
            html::push_html(&mut body, events.drain(..));
            if !cells.is_empty() {
                let mut cursor = 0;
                for offset in cells.drain(..) {
                    let next = body[cursor..].match_indices('<').find_map(|(i, _)| {
                        let at = cursor + i;
                        let tag = body.get(at + 1..at + 3)?;
                        let next = body.as_bytes().get(at + 3)?;
                        (matches!(tag, "th" | "td") && matches!(next, b' ' | b'>'))
                            .then_some(at + 3)
                    });
                    if let Some(at) = next {
                        let attr = format!(" data-source=\"{offset}\"");
                        body.insert_str(at, &attr);
                        cursor = at + attr.len();
                    }
                }
            }
            let next = utf16 + source[start..end].encode_utf16().count();
            out.push_str(&format!("<section class=\"block\" tabindex=\"0\" data-start=\"{utf16}\" data-end=\"{next}\">{body}</section>"));
            start = end;
            utf16 = next;
        }
    }
    if start < source.len() || out.is_empty() {
        let next = utf16 + source[start..].encode_utf16().count();
        out.push_str(&format!("<section class=\"block blank\" tabindex=\"0\" data-start=\"{utf16}\" data-end=\"{next}\"><br></section>"));
    }
    out
}

pub fn page(source: &str, epoch: u64, caret: usize, read_only: bool, has_folder: bool) -> String {
    let base = if has_folder {
        format!("<base href=\"https://{DOC_HOST}/\">")
    } else {
        String::new()
    };
    let assets = format!("https://{PREVIEW_HOST}/assets");
    format!(
        r#"<!doctype html><html lang="ja"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src https://{PREVIEW_HOST}; style-src https://{PREVIEW_HOST} 'unsafe-inline'; img-src https: data:; font-src https://{PREVIEW_HOST}; base-uri https://{DOC_HOST}; form-action 'none'">
{base}<link rel="stylesheet" href="{assets}/live.css"><script defer src="{assets}/live.js"></script>
</head><body data-epoch="{epoch}" data-caret="{caret}" data-readonly="{read_only}">
<header><strong>Markdown ライブ編集</strong><nav aria-label="書式">
<button data-format="heading">見出し</button><button data-format="bold"><b>太字</b></button><button data-format="italic"><i>斜体</i></button><button data-format="strike">取消線</button><button data-format="code">コード</button><button data-format="link">リンク</button><button data-format="list">箇条書き</button><button data-format="quote">引用</button><button data-format="fence">コード枠</button><button data-format="table">表</button></nav>
<nav aria-label="表の編集" id="table-tools"><span id="table-position">表のセルを選択</span><button data-table="column-before" disabled>左に列追加</button><button data-table="column-after" disabled>右に列追加</button><button data-table="delete-column" disabled>列削除</button><button data-table="row-before" disabled>上に行追加</button><button data-table="row-after" disabled>下に行追加</button><button data-table="delete-row" disabled>行削除</button></nav>
<button data-action="all">全体の原文</button><button data-command="source">通常編集に戻る</button>
<span id="hint">段落をクリックして編集 · Ctrl+B 太字 · Ctrl+I 斜体 · Ctrl+S 保存</span></header>
<main id="blocks" aria-label="Markdown 文書">{}</main><textarea id="source" hidden>{}</textarea>
</body></html>"#,
        blocks(source),
        escape(source)
    )
}

pub fn rendered(epoch: u64, seq: u64, source: &str) -> String {
    rendered_at(epoch, seq, source, None)
}

pub fn rendered_at(epoch: u64, seq: u64, source: &str, caret: Option<usize>) -> String {
    let context = table_context(source, caret);
    format!(
        "{{\"epoch\":{epoch},\"seq\":{seq},\"table\":{context},\"html\":{}}}",
        json_string(&blocks(source))
    )
}

pub fn replaced(previous: u64, epoch: u64, source: &str, caret: usize) -> String {
    format!(
        "{{\"rebase\":{previous},\"epoch\":{epoch},\"initial\":{},\"html\":{},\"caret\":{},\"table\":{}}}",
        json_string(source),
        json_string(&blocks(source)),
        source[..caret].encode_utf16().count(),
        table_context(source, Some(caret))
    )
}

fn table_context(source: &str, caret: Option<usize>) -> String {
    caret
        .and_then(|caret| table::context(source, caret))
        .map_or_else(
            || "null".to_owned(),
            |c| {
                format!(
                    "{{\"row\":{},\"column\":{},\"rows\":{},\"columns\":{}}}",
                    c.row, c.column, c.rows, c.columns
                )
            },
        )
}

/// UTF-16 の途中（サロゲートペアの間）は手前の文字境界に丸める。
pub fn byte_offset(source: &str, utf16: usize) -> usize {
    let mut units = 0;
    for (offset, c) in source.char_indices() {
        if units + c.len_utf16() > utf16 {
            return offset;
        }
        units += c.len_utf16();
    }
    source.len()
}

/// 共通の接頭・接尾を残し、変更した部分だけを文書の Undo に積む。
pub fn difference<'a>(before: &str, after: &'a str) -> Option<(Range<usize>, &'a str)> {
    if before == after {
        return None;
    }
    let mut prefix = before
        .bytes()
        .zip(after.bytes())
        .take_while(|(a, b)| a == b)
        .count();
    while !before.is_char_boundary(prefix) || !after.is_char_boundary(prefix) {
        prefix -= 1;
    }
    let mut suffix = before[prefix..]
        .bytes()
        .rev()
        .zip(after[prefix..].bytes().rev())
        .take_while(|(a, b)| a == b)
        .count();
    while !before.is_char_boundary(before.len() - suffix)
        || !after.is_char_boundary(after.len() - suffix)
    {
        suffix -= 1;
    }
    Some((
        prefix..before.len() - suffix,
        &after[prefix..after.len() - suffix],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_source_and_unicode_boundaries() {
        for (old, new) in [
            ("日本語😀末尾", "日本語😃末尾"),
            ("", "# 見出し\r\n"),
            ("abc", ""),
            ("é", "ê"),
        ] {
            let (r, inserted) = difference(old, new).unwrap();
            let mut actual = old.to_owned();
            actual.replace_range(r, inserted);
            assert_eq!(actual, new);
        }
        assert!(difference("same", "same").is_none());
        assert_eq!(byte_offset("あ😀z", 2), 3);
        assert_eq!(byte_offset("あ😀z", 3), 7);
        let source = "# 日本語😀\r\n\r\n- **太字**\r\n- [ ] task\r\n\r\n```js\r\ncode\r\n```\r\n";
        let body = blocks(source);
        assert!(body.contains("<h1>日本語😀</h1>"));
        assert!(body.contains("<strong>太字</strong>"));
        assert!(body.contains(&format!("data-end=\"{}\"", source.encode_utf16().count())));
        assert!(page(source, 9, 0, false, true).contains(&escape(source)));
    }
    #[test]
    fn editing_page_never_executes_document_html() {
        let source = "<script>window.chrome.webview.postMessage('bad')</script>\n\n<img onerror=alert(1) src=x>";
        let body = blocks(source);
        assert!(!body.contains("<script>"));
        assert!(!body.contains("<img onerror"));
        assert!(body.contains("&lt;script&gt;"));
        assert!(page(source, 1, 0, false, false).contains("Content-Security-Policy"));
    }
}
