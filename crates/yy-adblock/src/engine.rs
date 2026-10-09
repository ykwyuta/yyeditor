//! adblock クレートの包み（20 章 4・5）。
//!
//! * 要求の照合: URL・ページの URL・種類（`image`・`script`・`sub_frame` など）。
//! * 広告の枠を隠す: ページのホスト向けの規則と、ページにある class・id に当てはまる汎用の規則を
//!   CSS（`display: none !important`）にする。規則 1 つを 1 つの CSS の規則にする（書けない規則が
//!   1 つあっても、ほかが効くように）。
//! * ページに差し込むスクリプト: class・id を集めて `chrome.webview.postMessage` で送るもの（後から
//!   足された要素は MutationObserver で間引いて送る）と、CSS を差し込むもの。

use std::collections::HashSet;

use adblock::Engine;
use adblock::lists::{FilterSet, ParseOptions};
use adblock::request::Request;

/// ページから送られる class・id のメッセージの頭。
pub const MESSAGE_PREFIX: &str = "yyab\n";
/// 1 つのメッセージで受け取る class・id の上限（それぞれ）。
const MAX_TOKENS: usize = 5000;

/// 組み立てたエンジンと、その元の情報。
pub struct AdBlocker {
    engine: Engine,
    /// 規則の行の数（すべてのリストの合計）
    pub rules: usize,
    /// 使ったリストの名前
    pub lists: Vec<String>,
}

/// ページのホスト向けの非表示の情報（タブに覚えておき、汎用の規則を調べるときに使う）。
#[derive(Clone, Debug, Default)]
pub struct PageCosmetic {
    /// ホスト向けの規則の CSS（空なら差し込まない）
    pub css: String,
    /// 汎用の規則を使わない（`#@#` の `generichide`）
    pub generichide: bool,
    /// 例外の選択子（`#@#.ad`）
    pub exceptions: HashSet<String>,
}

impl AdBlocker {
    /// リスト（名前, 本文）からエンジンを作る。
    pub fn build(sources: Vec<(String, String)>) -> AdBlocker {
        let mut set = FilterSet::new(false);
        let mut rules = 0;
        let mut lists = Vec::new();
        for (name, text) in sources {
            rules += crate::lists::count_rules(&text);
            set.add_filter_list(text, ParseOptions::default());
            lists.push(name);
        }
        AdBlocker {
            engine: Engine::new_with_filter_set(set),
            rules,
            lists,
        }
    }

    /// この要求を止めるか。`kind` は adblock の種類の名前（`image`・`script`・`stylesheet`・`font`・
    /// `media`・`xmlhttprequest`・`websocket`・`ping`・`sub_frame`・`other`）。
    pub fn should_block(&self, url: &str, page_url: &str, kind: &str) -> bool {
        let lower = url.get(..8).unwrap_or(url).to_ascii_lowercase();
        if !(lower.starts_with("http://")
            || lower.starts_with("https://")
            || lower.starts_with("ws"))
        {
            return false;
        }
        match Request::new(url, page_url, kind, "get") {
            Ok(req) => self.engine.check_network_request(&req).should_block(),
            Err(_) => false,
        }
    }

    /// ページのホスト向けの非表示の規則。
    pub fn page_cosmetic(&self, page_url: &str) -> PageCosmetic {
        let r = self.engine.url_cosmetic_resources(page_url);
        PageCosmetic {
            css: css_of(r.hide_selectors.iter().map(String::as_str)),
            generichide: r.generichide,
            exceptions: r.exceptions,
        }
    }

    /// ページにある class・id に当てはまる汎用の規則の CSS（なければ空）。
    pub fn generic_css(&self, classes: &[String], ids: &[String], page: &PageCosmetic) -> String {
        if page.generichide {
            return String::new();
        }
        let sel = self
            .engine
            .hidden_class_id_selectors(classes, ids, &page.exceptions);
        css_of(sel.iter().map(String::as_str))
    }
}

/// 選択子を CSS に（1 つずつ別の規則。並びは決まった順）。
fn css_of<'a>(selectors: impl Iterator<Item = &'a str>) -> String {
    let mut v: Vec<&str> = selectors
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.contains(['{', '}']))
        .collect();
    v.sort_unstable();
    v.dedup();
    let mut css = String::new();
    for s in v {
        css.push_str(s);
        css.push_str("{display:none!important}\n");
    }
    css
}

/// JavaScript の文字列の中身にする（`"` で囲む）。
fn js_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '<' => out.push_str("\\u003c"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// CSS を差し込むスクリプト（空なら `None`）。
pub fn inject_css_script(css: &str) -> Option<String> {
    if css.is_empty() {
        return None;
    }
    Some(format!(
        "(()=>{{const s=document.createElement('style');s.setAttribute('data-yyab','');\
         s.textContent={};(document.head||document.documentElement).appendChild(s);}})();",
        js_string(css)
    ))
}

/// class・id を集めて送るスクリプト（DOMContentLoaded で 1 度。後から足された要素は 0.5 秒ごとに
/// まとめて、新しいものだけを送る）。
pub const COLLECTOR_SCRIPT: &str = r#"(()=>{
if (window.__yyab || !window.chrome || !chrome.webview) return; window.__yyab = true;
const seenC = new Set(), seenI = new Set();
let cs = [], is = [];
const visit = (e) => {
  if (e.id && !seenI.has(e.id)) { seenI.add(e.id); is.push(e.id); }
  if (e.classList) for (const c of e.classList) if (!seenC.has(c)) { seenC.add(c); cs.push(c); }
};
const scan = (root) => {
  if (root.nodeType !== 1) return;
  visit(root);
  for (const e of root.querySelectorAll('[class],[id]')) visit(e);
};
const flush = () => {
  if (cs.length || is.length) chrome.webview.postMessage('yyab\n' + cs.join(' ') + '\n' + is.join(' '));
  cs = []; is = [];
};
scan(document.documentElement); flush();
let pending = [], timer = 0;
new MutationObserver((ms) => {
  for (const m of ms) for (const n of m.addedNodes) if (n.nodeType === 1) pending.push(n);
  if (pending.length > 2000) pending = [document.documentElement];
  if (!timer) timer = setTimeout(() => { timer = 0; const p = pending; pending = []; for (const n of p) scan(n); flush(); }, 500);
}).observe(document.documentElement, { childList: true, subtree: true });
})();"#;

/// ページからのメッセージ（`yyab\n<class …>\n<id …>`）を読む。違う形なら `None`。
pub fn parse_message(msg: &str) -> Option<(Vec<String>, Vec<String>)> {
    let body = msg.strip_prefix(MESSAGE_PREFIX)?;
    let (classes, ids) = body.split_once('\n').unwrap_or((body, ""));
    let tokens = |s: &str| -> Vec<String> {
        s.split(' ')
            .filter(|t| !t.is_empty() && t.len() <= 256 && !t.chars().any(char::is_whitespace))
            .take(MAX_TOKENS)
            .map(str::to_owned)
            .collect()
    };
    Some((tokens(classes), tokens(ids)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blocker(text: &str) -> AdBlocker {
        AdBlocker::build(vec![("test".into(), text.into())])
    }

    #[test]
    fn blocks_network_requests() {
        let b = blocker(
            "! Expires: 4 days\n||ads.example^\n/banner/*$image\n||tracker.example^$third-party\n@@||ads.example/allowed.js\n",
        );
        assert_eq!(b.rules, 4);
        let page = "https://news.example.jp/article";
        assert!(b.should_block("https://ads.example/x.js", page, "script"));
        assert!(!b.should_block("https://ads.example/allowed.js", page, "script"));
        assert!(b.should_block("https://cdn.example.jp/banner/1.png", page, "image"));
        assert!(!b.should_block("https://cdn.example.jp/banner/1.js", page, "script"));
        assert!(b.should_block("https://tracker.example/p.gif", page, "image"));
        // 同じサイトの中なら第三者の規則は当てはまらない
        assert!(!b.should_block(
            "https://tracker.example/p.gif",
            "https://tracker.example/",
            "image"
        ));
        assert!(!b.should_block("https://news.example.jp/style.css", page, "stylesheet"));
        assert!(!b.should_block("data:image/png;base64,AAAA", page, "image"));
        assert!(!b.should_block("not a url", page, "other"));
    }

    #[test]
    fn hides_elements() {
        let b =
            blocker("news.example.jp##.side-ad\n##.ad-box\n###top-ad\nother.example#@#.ad-box\n");
        let page = b.page_cosmetic("https://news.example.jp/");
        assert_eq!(page.css, ".side-ad{display:none!important}\n");
        let css = b.generic_css(
            &["ad-box".into(), "content".into()],
            &["top-ad".into()],
            &page,
        );
        assert!(css.contains(".ad-box{display:none!important}"), "{css}");
        assert!(css.contains("#top-ad{display:none!important}"), "{css}");
        assert!(!css.contains("content"));
        // 例外のあるサイトでは隠さない
        let other = b.page_cosmetic("https://other.example/");
        assert!(other.css.is_empty());
        let css = b.generic_css(&["ad-box".into()], &[], &other);
        assert!(!css.contains(".ad-box"), "{css}");
    }

    #[test]
    fn scripts_and_messages() {
        assert!(inject_css_script("").is_none());
        let s = inject_css_script("a[title=\"x\"]{display:none!important}\n</style>").unwrap();
        assert!(
            s.contains(r#"a[title=\"x\"]{display:none!important}\n\u003c/style>"#),
            "{s}"
        );
        assert!(!s.contains("</style>"));
        assert_eq!(js_string("a\u{2028}\u{1}"), "\"a\\u2028\\u0001\"");
        assert_eq!(
            parse_message("yyab\nad-box  content\ntop-ad"),
            Some((
                vec!["ad-box".into(), "content".into()],
                vec!["top-ad".into()]
            ))
        );
        assert_eq!(
            parse_message("yyab\nonly"),
            Some((vec!["only".into()], vec![]))
        );
        assert_eq!(parse_message("other"), None);
        let many = format!("yyab\n{}\n", "c ".repeat(MAX_TOKENS + 10));
        assert_eq!(parse_message(&many).unwrap().0.len(), MAX_TOKENS);
        assert!(COLLECTOR_SCRIPT.contains("chrome.webview.postMessage('yyab\\n'"));
    }

    #[test]
    fn engine_is_send() {
        fn send<T: Send + Sync>() {}
        send::<AdBlocker>();
    }
}
