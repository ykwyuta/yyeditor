//! Markdown の変換とページ・埋め込みファイルのテスト。

use std::path::Path;

use yy_preview::{Kind, PageOptions, asset, html_page, markdown_page, markdown_to_html};

fn md(src: &str) -> String {
    markdown_to_html(src, None)
}

#[test]
fn extended_syntax() {
    let html = md("| a | b |\n|---|:-:|\n| 1 | 2 |\n\n\
         ~~del~~ ^sup^ ~sub~\n\n\
         - [x] done\n- [ ] todo\n\n\
         term\n: definition\n\n\
         note[^1]\n\n[^1]: footnote text\n\n\
         > [!WARNING]\n> careful\n");
    assert!(html.contains("<table>"), "{html}");
    assert!(
        html.contains("<th style=\"text-align: center\">b</th>"),
        "{html}"
    );
    assert!(html.contains("<del>del</del>"), "{html}");
    assert!(html.contains("<sup>sup</sup>"), "{html}");
    assert!(html.contains("<sub>sub</sub>"), "{html}");
    assert!(
        html.contains("<input disabled=\"\" type=\"checkbox\" checked=\"\"/>"),
        "{html}"
    );
    assert!(html.contains("<dt>term</dt>"), "{html}");
    assert!(html.contains("<dd>definition</dd>"), "{html}");
    assert!(html.contains("footnote-definition"), "{html}");
    assert!(html.contains("class=\"markdown-alert-warning\""), "{html}");
}

#[test]
fn soft_breaks_between_japanese_are_joined() {
    let html = md("日本語の文です。\n続きの文。\nEnglish\nwords\n");
    assert!(
        html.contains("日本語の文です。続きの文。\nEnglish\nwords"),
        "{html}"
    );
}

#[test]
fn footnotes_are_collected_at_the_end() {
    let html = md("a[^n]\n\n[^n]: note body\n\nlast paragraph\n");
    let note = html.find("note body").unwrap();
    let last = html.find("last paragraph").unwrap();
    assert!(last < note, "{html}");
    assert!(html.contains("<section class=\"footnotes\">"), "{html}");
}

#[test]
fn mermaid_and_math() {
    let html = md("```mermaid\ngraph TD\n  A-->B<C\n```\n\n\
         inline $a^2 + b^2$ and\n\n$$\\int_0^1 x\\,dx$$\n\n```math\n\\frac{1}{2}\n```\n");
    assert!(
        html.contains("<pre class=\"mermaid\" data-line=\"0\">graph TD\n  A--&gt;B&lt;C\n</pre>"),
        "{html}"
    );
    assert!(
        html.contains("<span class=\"math math-inline\">a^2 + b^2</span>"),
        "{html}"
    );
    assert!(
        html.contains("<span class=\"math math-display\">\\int_0^1 x\\,dx</span>"),
        "{html}"
    );
    assert!(
        html.contains("<div class=\"math math-display\" data-line=\"9\">\\frac{1}{2}\n</div>"),
        "{html}"
    );
}

#[test]
fn heading_ids_follow_github_rules() {
    let html = md(
        "# Hello, World!\n\n## 日本語 の見出し\n\n# Hello, World!\n\n### Custom {#my-id .cls}\n",
    );
    assert!(
        html.contains("<h1 id=\"hello-world\" data-line=\"0\">Hello, World!</h1>"),
        "{html}"
    );
    assert!(
        html.contains("<h2 id=\"日本語-の見出し\" data-line=\"2\">"),
        "{html}"
    );
    assert!(
        html.contains("<h1 id=\"hello-world-1\" data-line=\"4\">"),
        "{html}"
    );
    assert!(
        html.contains("<h3 id=\"my-id\" class=\"cls\" data-line=\"6\">Custom</h3>"),
        "{html}"
    );
}

#[test]
fn top_level_blocks_carry_source_lines() {
    let html = md("para one\n\n- item\n- item\n\n> quote\n\n---\n\n| a |\n|---|\n| b |\n");
    for line in [0, 2, 5, 7, 9] {
        assert!(
            html.contains(&format!(
                "<span class=\"yy-line\" data-line=\"{line}\"></span>"
            )),
            "line {line}: {html}"
        );
    }
    // 入れ子のブロック・表の中には付けない
    assert_eq!(html.matches("yy-line").count(), 5, "{html}");
}

#[test]
fn code_blocks_are_highlighted_and_escaped() {
    let reg = yy_syntax::Registry::builtin();
    let html = markdown_to_html(
        "```rust\nfn main() { let x = \"<a>\"; }\n```\n\n```\n<plain>\n```\n",
        Some(&reg),
    );
    assert!(html.contains("<code class=\"language-rust\">"), "{html}");
    assert!(html.contains("<span class=\"tok-keyword"), "{html}");
    assert!(html.contains("&quot;&lt;a&gt;&quot;"), "{html}");
    assert!(
        html.contains("<pre data-line=\"4\"><code>&lt;plain&gt;\n</code></pre>"),
        "{html}"
    );
    // 知らない言語はそのまま
    let html = markdown_to_html("```nosuchlang\na<b\n```\n", Some(&reg));
    assert!(
        html.contains("<code class=\"language-nosuchlang\">a&lt;b\n</code>"),
        "{html}"
    );
}

#[test]
fn front_matter_is_shown_as_code() {
    let html = md("---\ntitle: x\n---\n\n# Body\n");
    assert!(
        html.contains("<pre class=\"front-matter\" data-line=\"0\">"),
        "{html}"
    );
    assert!(html.contains("title: x"), "{html}");
    assert!(html.contains("<h1 id=\"body\""), "{html}");
}

#[test]
fn pages_reference_bundled_assets() {
    let page = markdown_page(
        "<p>x</p>",
        &PageOptions {
            has_folder: true,
            token_colors: vec![
                ("keyword.control".into(), "#111111".into()),
                ("keyword".into(), "#222222".into()),
                ("bad".into(), "red;}body{".into()),
            ],
        },
    );
    assert!(page.contains("<base href=\"https://yy-doc.local/\">"));
    assert!(page.contains("<main id=\"yy-content\"><p>x</p></main>"));
    // 細分類の色が後（優先）
    let k = page.find("pre .tok-keyword{color:#222222}").unwrap();
    let kc = page
        .find("pre .tok-keyword-control{color:#111111}")
        .unwrap();
    assert!(k < kc);
    assert!(!page.contains("red;"));
    for src in [
        "katex.min.css",
        "preview.css",
        "katex.min.js",
        "mermaid.min.js",
        "preview.js",
    ] {
        let path = format!("/assets/{src}");
        assert!(
            page.contains(&format!("https://yy-preview.local{path}")),
            "{src}"
        );
        assert!(asset(&path).is_some(), "{src}");
    }
}

#[test]
fn embedded_assets_decompress() {
    let (js, mime) = asset("/assets/mermaid.min.js").unwrap();
    assert!(mime.starts_with("text/javascript"));
    assert!(js.len() > 1_000_000);
    assert!(String::from_utf8_lossy(&js[js.len() - 200..]).contains("mermaid"));
    let (css, _) = asset("/assets/katex.min.css").unwrap();
    let css = String::from_utf8_lossy(&css);
    // KaTeX の CSS が参照する woff2 フォントはすべて埋め込んである
    let mut n = 0;
    for part in css.split("url(fonts/").skip(1) {
        let name = &part[..part.find(')').unwrap()];
        if name.ends_with(".woff2") {
            n += 1;
            assert!(asset(&format!("/assets/fonts/{name}")).is_some(), "{name}");
        }
    }
    assert!(n >= 20, "{n}");
    assert!(asset("/assets/nothing.js").is_none());
    assert!(asset("/other").is_none());
}

#[test]
fn html_page_gets_base_and_scroll_script() {
    let opts = PageOptions {
        has_folder: true,
        ..Default::default()
    };
    let page = html_page(
        "<!DOCTYPE html><HTML><Head><title>t</title></Head><body>x</body></HTML>",
        &opts,
    );
    assert!(
        page.starts_with(
            "<!DOCTYPE html><HTML><Head><base href=\"https://yy-doc.local/\"><script>"
        ),
        "{page}"
    );
    let page = html_page("<p>fragment</p>", &PageOptions::default());
    assert!(page.starts_with("<script>"), "{page}");
    assert!(page.ends_with("<p>fragment</p>"));
}

#[test]
fn detects_kind() {
    assert_eq!(Kind::detect(Some("markdown"), None), Some(Kind::Markdown));
    assert_eq!(
        Kind::detect(None, Some(Path::new("a/README.MD"))),
        Some(Kind::Markdown)
    );
    assert_eq!(
        Kind::detect(Some("html"), Some(Path::new("x.txt"))),
        Some(Kind::Html)
    );
    assert_eq!(
        Kind::detect(None, Some(Path::new("index.htm"))),
        Some(Kind::Html)
    );
    assert_eq!(Kind::detect(Some("rust"), Some(Path::new("main.rs"))), None);
    assert_eq!(Kind::detect(None, None), None);
}

#[test]
fn update_message_is_valid_json() {
    let m = yy_preview::update_message(Some("<p a=\"1\">\\\n\u{2028}</p>"), Some(3));
    assert_eq!(
        m,
        "{\"html\":\"<p a=\\\"1\\\">\\\\\\n\\u2028</p>\",\"line\":3}"
    );
    assert_eq!(yy_preview::update_message(None, Some(0)), "{\"line\":0}");
}
