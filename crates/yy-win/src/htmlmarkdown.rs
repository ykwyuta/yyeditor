//! Pure Rust HTML conversion and byte-oriented Windows CF_HTML extraction.

pub(crate) fn convert(html: &str) -> Result<String, String> {
    htmd::HtmlToMarkdown::builder()
        .skip_tags(vec!["head", "script", "style", "noscript", "template"])
        .build()
        .convert(html)
        .map_err(|e| format!("HTMLをMarkdownに変換できませんでした: {e}"))
}

/// CF_HTML offsets count UTF-8 bytes from the beginning of the clipboard data.
/// Never interpret the header or unselected surrounding document as content.
fn fragment(bytes: &[u8]) -> Option<&str> {
    let bytes = bytes.split(|b| *b == 0).next()?;
    let header_end = bytes.iter().position(|b| *b == b'<').unwrap_or(bytes.len());
    let header = std::str::from_utf8(&bytes[..header_end]).ok()?;
    let offset = |name: &str| {
        header.lines().find_map(|line| {
            line.strip_prefix(name)
                .and_then(|v| v.trim().parse::<usize>().ok())
        })
    };
    let slice = match (offset("StartFragment:"), offset("EndFragment:")) {
        (Some(start), Some(end)) if start >= header_end && start <= end => bytes.get(start..end)?,
        (Some(_), Some(_)) => return None,
        _ => {
            // Some producers use -1 offsets and explicit fragment markers.
            let html = std::str::from_utf8(&bytes[header_end..]).ok()?;
            let (_, rest) = html.split_once("<!--StartFragment-->")?;
            let (selected, _) = rest.split_once("<!--EndFragment-->")?;
            return Some(selected);
        }
    };
    std::str::from_utf8(slice).ok()
}

pub(crate) fn clipboard_markdown(
    owner: windows::Win32::Foundation::HWND,
) -> Result<String, String> {
    use crate::clipboard;
    let format = clipboard::named_format("HTML Format");
    let html = if unsafe {
        windows::Win32::System::DataExchange::IsClipboardFormatAvailable(format).is_ok()
    } {
        let bytes = clipboard::get_data(owner, format, 16 << 20)
            .ok_or("クリップボードのHTMLを読み取れませんでした（上限16 MiB）。")?;
        fragment(&bytes)
            .ok_or("クリップボードのHTML形式が不正です。")?
            .to_owned()
    } else {
        let (text, _) =
            clipboard::get_text(owner).ok_or("クリップボードにHTMLまたはテキストがありません。")?;
        if text.len() > 16 << 20 {
            return Err("変換できるHTMLは16 MiBまでです。".into());
        }
        text
    };
    convert(&html)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_markdown_structures_and_entities() {
        let md = convert(concat!(
            "<h1>日本語 &amp; HTML</h1><p><strong>太字</strong> <em>斜体</em> ",
            "<a href=\"https://example.com\">リンク</a></p>",
            "<ul><li>項目</li><li>次</li></ul><blockquote><p>引用</p></blockquote>",
            "<pre><code>let x = 1;\n</code></pre>",
            "<table><tr><th>名前</th><th>値</th></tr><tr><td>あ</td><td>1</td></tr></table>",
            "<script>unwanted()</script><style>unwanted</style>"
        ))
        .unwrap();
        for expected in [
            "# 日本語 & HTML",
            "**太字**",
            "*斜体*",
            "[リンク](https://example.com)",
            "項目",
            "> 引用",
            "```",
            "let x = 1;",
            "| 名前 | 値 |",
            "| あ  | 1 |",
        ] {
            assert!(md.contains(expected), "missing {expected:?}: {md}");
        }
        assert!(!md.contains("unwanted"));
    }

    #[test]
    fn html_markdown_recovers_incomplete_html() {
        assert_eq!(
            convert("<p>日本語 &lt; &gt; &#x1f600;<br><b>太字").unwrap(),
            "日本語 < > 😀  \n**太字**"
        );
        assert_eq!(convert("<script>only script</script>").unwrap(), "");
    }

    #[test]
    fn html_markdown_paste_replaces_selection_and_undoes_once() {
        use yy_core::{Document, Selection, SelectionSet};
        for newline in ["\n", "\r\n"] {
            let original = format!("前 selected 後{newline}");
            let mut doc = Document::from_text(&original);
            doc.set_selections(SelectionSet::single(Selection {
                anchor: "前 ".len() as u64,
                head: "前 selected".len() as u64,
                goal_x: None,
            }));
            let markdown = convert("<h2>日本語</h2><p><b>本文</b></p>").unwrap();
            assert!(doc.paste(&markdown));
            let pasted = format!("前 {} 後{newline}", markdown.replace('\n', newline));
            assert_eq!(
                doc.snapshot().read(0..doc.snapshot().len()),
                pasted.as_bytes()
            );
            assert!(doc.undo());
            assert_eq!(
                doc.snapshot().read(0..doc.snapshot().len()),
                original.as_bytes()
            );
            assert!(doc.redo());
            assert_eq!(
                doc.snapshot().read(0..doc.snapshot().len()),
                pasted.as_bytes()
            );
        }
    }

    #[test]
    fn html_markdown_cf_html_uses_byte_offsets() {
        let prefix = "<html><body>対象外<!--StartFragment-->";
        let selected = "<h2>選択した日本語</h2>";
        let suffix = "<!--EndFragment-->対象外</body></html>";
        let header = |start, end| {
            format!("Version:1.0\r\nStartFragment:{start:010}\r\nEndFragment:{end:010}\r\n")
        };
        let start = header(0, 0).len() + prefix.len();
        let data = format!(
            "{}{prefix}{selected}{suffix}\0",
            header(start, start + selected.len())
        );
        assert_eq!(fragment(data.as_bytes()), Some(selected));
        assert_eq!(
            convert(fragment(data.as_bytes()).unwrap()).unwrap(),
            "## 選択した日本語"
        );
    }

    #[test]
    fn html_markdown_cf_html_rejects_bad_ranges_and_supports_markers() {
        for data in [
            "StartFragment:999\nEndFragment:1000\n<p>x</p>",
            "StartFragment:50\nEndFragment:1\n<p>x</p>",
            "Version:1.0\n<p>x</p>",
        ] {
            assert_eq!(fragment(data.as_bytes()), None);
        }
        assert_eq!(fragment(b"StartFragment:-1\r\nEndFragment:-1\r\n<html><!--StartFragment--><b>x</b><!--EndFragment--></html>"), Some("<b>x</b>"));
    }
}
