//! Markdown / HTML のプレビュー（エディタの右側に表示する）の、画面に依存しない部分。
//!
//! * [`markdown_to_html`]: Markdown を HTML にする（拡張構文・mermaid・数式に対応）
//! * [`markdown_page`] / [`html_page`]: プレビューに表示するページ全体
//! * [`asset`]: ページが読み込む mermaid・KaTeX などの埋め込みファイル
//!
//! ページとファイルは `https://yy-preview.local/` から配信する（Windows では WebView2 の
//! リソース要求を横取りして返す）。文書と同じフォルダの画像などは `https://yy-doc.local/`
//! （文書のフォルダを割り当てた仮想ホスト）から読む。

mod markdown;

use std::borrow::Cow;
use std::path::Path;
use std::sync::OnceLock;

pub use markdown::{escape, markdown_to_html, token_classes};

/// ページと埋め込みファイルを配信するホスト。
pub const PREVIEW_HOST: &str = "yy-preview.local";
/// 文書のフォルダを割り当てるホスト。
pub const DOC_HOST: &str = "yy-doc.local";
/// プレビューのページの URL。
pub const PAGE_URL: &str = "https://yy-preview.local/index.html";

/// プレビューの種類。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Markdown,
    Html,
}

impl Kind {
    /// ハイライトの定義の ID・ファイル名から種類を決める。プレビューできなければ `None`。
    pub fn detect(syntax_id: Option<&str>, path: Option<&Path>) -> Option<Kind> {
        match syntax_id {
            Some("markdown") => return Some(Kind::Markdown),
            Some("html") => return Some(Kind::Html),
            _ => {}
        }
        let ext = path?.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "md" | "markdown" | "mdown" | "mkd" | "mkdn" | "mdx" => Some(Kind::Markdown),
            "html" | "htm" | "xhtml" => Some(Kind::Html),
            _ => None,
        }
    }
}

/// ページの設定。
#[derive(Clone, Debug, Default)]
pub struct PageOptions {
    /// 文書のフォルダがあるか（相対パスを `https://yy-doc.local/` から読む）
    pub has_folder: bool,
    /// コードの色（トークン名 → `#RRGGBB`）
    pub token_colors: Vec<(String, String)>,
}

/// Markdown のプレビューのページ全体。本文（[`markdown_to_html`] の結果）は `#yy-content` に入る。
pub fn markdown_page(body: &str, opts: &PageOptions) -> String {
    let base = if opts.has_folder {
        format!("<base href=\"https://{DOC_HOST}/\">")
    } else {
        String::new()
    };
    let a = format!("https://{PREVIEW_HOST}/assets");
    format!(
        "<!DOCTYPE html>\n<html><head><meta charset=\"utf-8\">{base}\
         <meta name=\"color-scheme\" content=\"light\">\
         <link rel=\"stylesheet\" href=\"{a}/katex.min.css\">\
         <link rel=\"stylesheet\" href=\"{a}/preview.css\">\
         <style>{}</style>\
         <script src=\"{a}/katex.min.js\"></script>\
         <script src=\"{a}/mermaid.min.js\"></script>\
         <script src=\"{a}/preview.js\"></script>\
         </head><body><main id=\"yy-content\">{body}</main></body></html>\n",
        token_css(&opts.token_colors)
    )
}

/// HTML 文書のプレビューのページ（文書そのもの）。相対パスを文書のフォルダから読むように
/// `<base>` を入れ、再読み込みしてもスクロール位置を保つスクリプトを加える。
pub fn html_page(src: &str, opts: &PageOptions) -> String {
    let mut head = String::new();
    if opts.has_folder {
        head += &format!("<base href=\"https://{DOC_HOST}/\">");
    }
    head += "<script>(()=>{const k='yy-preview-scroll';\
             addEventListener('load',()=>{const y=sessionStorage.getItem(k);if(y)scrollTo(0,+y)});\
             addEventListener('scroll',()=>sessionStorage.setItem(k,scrollY));\
             document.addEventListener('click',e=>{const a=e.target.closest&&e.target.closest('a[href^=\"#\"]');\
             if(!a)return;e.preventDefault();const id=decodeURIComponent(a.getAttribute('href').slice(1));\
             const t=document.getElementById(id)||document.getElementsByName(id)[0];if(t)t.scrollIntoView()})\
             })()</script>";
    // <head> の直後（なければ先頭）に入れる。<base> は他の URL より前にないと効かない
    let lower = src.to_ascii_lowercase();
    let at = lower
        .find("<head")
        .and_then(|i| lower[i..].find('>').map(|j| i + j + 1))
        .or_else(|| {
            lower
                .find("<html")
                .and_then(|i| lower[i..].find('>').map(|j| i + j + 1))
        })
        .unwrap_or(0);
    let mut out = String::with_capacity(src.len() + head.len());
    out.push_str(&src[..at]);
    out.push_str(&head);
    out.push_str(&src[at..]);
    out
}

/// トークンの色の CSS（`keyword` より `keyword.control` を優先する）。
fn token_css(colors: &[(String, String)]) -> String {
    let mut sorted: Vec<&(String, String)> = colors.iter().collect();
    sorted.sort_by_key(|(name, _)| name.matches('.').count());
    let mut css = String::new();
    for (name, color) in sorted {
        let valid = color.len() == 7
            && color.starts_with('#')
            && color[1..].chars().all(|c| c.is_ascii_hexdigit());
        if !valid {
            continue;
        }
        let class = name
            .split('.')
            .map(markdown::css_ident)
            .collect::<Vec<_>>()
            .join("-");
        css += &format!("pre .tok-{class}{{color:{color}}}");
    }
    css
}

/// 埋め込みファイル（`/assets/…` のパス）の内容と MIME タイプ。
pub fn asset(path: &str) -> Option<(Cow<'static, [u8]>, &'static str)> {
    let name = path.strip_prefix("/assets/")?;
    macro_rules! compressed {
        ($file:literal) => {{
            static DATA: OnceLock<Vec<u8>> = OnceLock::new();
            Cow::Borrowed(
                DATA.get_or_init(|| {
                    miniz_oxide::inflate::decompress_to_vec_zlib(include_bytes!(concat!(
                        "../assets/",
                        $file,
                        ".z"
                    )))
                    .expect("corrupt embedded asset")
                })
                .as_slice(),
            )
        }};
    }
    let js = "text/javascript; charset=utf-8";
    let css = "text/css; charset=utf-8";
    Some(match name {
        "mermaid.min.js" => (compressed!("mermaid.min.js"), js),
        "katex.min.js" => (compressed!("katex.min.js"), js),
        "katex.min.css" => (compressed!("katex.min.css"), css),
        "preview.js" => (
            Cow::Borrowed(include_bytes!("../assets/preview.js").as_slice()),
            js,
        ),
        "preview.css" => (
            Cow::Borrowed(include_bytes!("../assets/preview.css").as_slice()),
            css,
        ),
        _ => {
            let font = name.strip_prefix("fonts/")?;
            (Cow::Borrowed(font_data(font)?), "font/woff2")
        }
    })
}

/// KaTeX のフォント（woff2）。
fn font_data(name: &str) -> Option<&'static [u8]> {
    macro_rules! fonts {
        ($($f:literal),* $(,)?) => {
            match name {
                $(concat!($f, ".woff2") => Some(include_bytes!(concat!("../assets/fonts/", $f, ".woff2")).as_slice()),)*
                _ => None,
            }
        };
    }
    fonts!(
        "KaTeX_AMS-Regular",
        "KaTeX_Caligraphic-Bold",
        "KaTeX_Caligraphic-Regular",
        "KaTeX_Fraktur-Bold",
        "KaTeX_Fraktur-Regular",
        "KaTeX_Main-Bold",
        "KaTeX_Main-BoldItalic",
        "KaTeX_Main-Italic",
        "KaTeX_Main-Regular",
        "KaTeX_Math-BoldItalic",
        "KaTeX_Math-Italic",
        "KaTeX_SansSerif-Bold",
        "KaTeX_SansSerif-Italic",
        "KaTeX_SansSerif-Regular",
        "KaTeX_Script-Regular",
        "KaTeX_Size1-Regular",
        "KaTeX_Size2-Regular",
        "KaTeX_Size3-Regular",
        "KaTeX_Size4-Regular",
        "KaTeX_Typewriter-Regular",
    )
}

/// 文書のフォルダのファイルの MIME タイプ（拡張子から）。
pub fn mime_for(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        _ => "application/octet-stream",
    }
}

/// エディタからページへ送るメッセージ（JSON）。`html` があれば本文を差し替え、`line` があれば
/// その行へスクロールする。
pub fn update_message(html: Option<&str>, line: Option<usize>) -> String {
    let mut parts = Vec::new();
    if let Some(h) = html {
        parts.push(format!("\"html\":{}", json_string(h)));
    }
    if let Some(l) = line {
        parts.push(format!("\"line\":{l}"));
    }
    format!("{{{}}}", parts.join(","))
}

/// JSON の文字列リテラル。
fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // U+2028・U+2029 は JavaScript の文字列に含められないことがある
            c if (c as u32) < 0x20 || c == '\u{2028}' || c == '\u{2029}' => {
                out.push_str(&format!("\\u{:04x}", c as u32))
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
