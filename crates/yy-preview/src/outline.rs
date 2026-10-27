//! Flat, ID-addressed outline documents rendered by the shared live editor bridge.
use crate::{PREVIEW_HOST, escape};

pub fn page(source: &str, epoch: u64, read_only: bool) -> String {
    format!(
        r#"<!doctype html><html lang="ja"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src https://{PREVIEW_HOST}; style-src https://{PREVIEW_HOST}; base-uri 'none'; form-action 'none'">
<link rel="stylesheet" href="https://{PREVIEW_HOST}/assets/outline.css">
<script defer src="https://{PREVIEW_HOST}/assets/outline-model.js"></script>
<script defer src="https://{PREVIEW_HOST}/assets/outline.js"></script></head>
<body data-epoch="{epoch}" data-readonly="{read_only}">
<header><strong>アウトライン</strong><button data-op="add">項目追加</button><button data-op="indent">階層を下げる</button><button data-op="outdent">階層を上げる</button><button data-op="up">上へ</button><button data-op="down">下へ</button><button data-op="bookmark">☆ ブックマーク</button><button data-op="reference">参照を追加</button><button data-op="delete">枝を削除</button><button data-command="save">保存</button><button data-command="undo">元に戻す</button><button data-command="redo">やり直す</button><button data-command="source">通常編集に戻る</button></header>
<p id="status" role="status">Enter 項目追加 · Tab / Shift+Tab 階層変更 · Alt+↑↓ 移動 · ● ズーム · Ctrl+S 保存</p>
<nav id="crumbs" aria-label="階層の経路"></nav><div class="layout"><aside><h2>ブックマーク</h2><div id="bookmarks"></div><h2>参照元</h2><div id="backlinks"></div></aside><main id="tree" aria-label="アウトライン"></main></div>
<textarea id="source" hidden>{}</textarea></body></html>"#,
        escape(source)
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn source_cannot_escape_the_inert_container() {
        let page = super::page("</textarea><script>alert(1)</script>", 7, true);
        assert!(page.contains("&lt;/textarea&gt;"));
        assert!(page.contains("data-readonly=\"true\""));
        assert!(!page.contains("<script>alert"));
    }
}
