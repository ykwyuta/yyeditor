// yyeditor のプレビュー: 数式（KaTeX）と図（mermaid）を描き、エディタからの更新と
// スクロール位置を受け取る。
(() => {
  "use strict";
  const content = () => document.getElementById("yy-content");
  // 同じ図を描き直さないように、図の元の文字列 → 描いた SVG を覚えておく
  const diagrams = new Map();
  let seq = 0;

  if (window.mermaid) {
    mermaid.initialize({
      startOnLoad: false,
      securityLevel: "strict",
      theme: "default",
      // 誤りのある図はその場に文字で示す（ページの末尾に誤りの図を足さない）
      suppressErrorRendering: true,
    });
  }

  function renderMath(root) {
    if (!window.katex) return;
    for (const el of root.querySelectorAll(".math:not([data-rendered])")) {
      const tex = el.textContent;
      try {
        katex.render(tex, el, {
          displayMode: el.classList.contains("math-display"),
          throwOnError: false,
        });
      } catch (e) {
        el.textContent = tex;
        el.classList.add("yy-error");
        el.title = String(e);
      }
      el.setAttribute("data-rendered", "");
    }
  }

  async function renderDiagrams(root) {
    if (!window.mermaid) return;
    for (const el of root.querySelectorAll("pre.mermaid:not([data-rendered])")) {
      const src = el.textContent;
      el.setAttribute("data-rendered", "");
      let svg = diagrams.get(src);
      if (svg === undefined) {
        const id = "yy-mermaid-" + ++seq;
        try {
          svg = (await mermaid.render(id, src)).svg;
        } catch (e) {
          svg = null;
          el.classList.add("yy-error");
          el.setAttribute("data-error", String(e && e.message ? e.message : e));
        }
        // 描画に使った一時的な要素が残っていれば消す
        for (const tmp of [document.getElementById("d" + id), document.getElementById(id)]) {
          if (tmp && !el.contains(tmp)) tmp.remove();
        }
        if (svg) diagrams.set(src, svg);
      }
      if (svg) {
        el.innerHTML = svg;
        el.classList.add("yy-diagram");
      }
    }
    // 使われなくなった図を忘れる
    if (diagrams.size > 200) diagrams.clear();
    document.body.setAttribute("data-diagrams-done", "");
  }

  function render(root) {
    renderMath(root);
    return renderDiagrams(root);
  }

  // エディタの行 `line`（0 始まり）に対応する位置へスクロールする
  function scrollToLine(line) {
    const marks = content().querySelectorAll("[data-line]");
    let before = null;
    let after = null;
    for (const m of marks) {
      const l = +m.getAttribute("data-line");
      if (l <= line) before = { l, m };
      else {
        after = { l, m };
        break;
      }
    }
    const top = (el) => el.getBoundingClientRect().top + window.scrollY;
    let y = 0;
    if (before) {
      y = top(before.m);
      if (after && after.l > before.l) {
        y += ((line - before.l) / (after.l - before.l)) * (top(after.m) - y);
      }
    }
    window.scrollTo(0, Math.max(0, y - 8));
  }

  // 最後に指定された行（図を描き終えて高さが変わったら合わせ直す）
  let lastLine = null;

  async function update(msg) {
    if (typeof msg === "string") msg = JSON.parse(msg);
    if (msg.line !== undefined) lastLine = msg.line;
    if (msg.html !== undefined) {
      const y = window.scrollY;
      content().innerHTML = msg.html;
      renderMath(content());
      window.scrollTo(0, y);
      if (lastLine !== null) scrollToLine(lastLine);
      await renderDiagrams(content());
    }
    if (lastLine !== null) scrollToLine(lastLine);
  }

  // ページ内のリンク（#見出し）。<base> で文書のフォルダを指しているため、そのままでは
  // 別のページへの移動になってしまう
  document.addEventListener("click", (e) => {
    const a = e.target.closest && e.target.closest('a[href^="#"]');
    if (!a) return;
    e.preventDefault();
    const id = decodeURIComponent(a.getAttribute("href").slice(1));
    const target = document.getElementById(id) || document.getElementsByName(id)[0];
    if (target) target.scrollIntoView();
  });

  window.yyPreview = { update, render, scrollToLine };
  if (window.chrome && window.chrome.webview) {
    window.chrome.webview.addEventListener("message", (e) => update(e.data));
  }
  const start = () => render(content());
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", start);
  else start();
})();
