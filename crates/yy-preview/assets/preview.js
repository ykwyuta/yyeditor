// yyeditor のプレビュー: 数式（KaTeX）と図（mermaid・d3）を描き、エディタからの更新と
// スクロール位置を受け取る。
(() => {
  "use strict";
  // 埋め込みファイルの場所（このスクリプトと同じフォルダ）
  const assets = document.currentScript
    ? document.currentScript.src.replace(/[^/]*$/, "")
    : "https://yy-preview.local/assets/";
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

  // d3.js は ```d3 のブロックがあるときだけ読み込む
  let d3Loading = null;
  function loadD3() {
    if (window.d3) return Promise.resolve();
    if (!d3Loading) {
      d3Loading = new Promise((resolve, reject) => {
        const s = document.createElement("script");
        s.src = assets + "d3.min.js";
        s.onload = () => resolve();
        s.onerror = () => {
          d3Loading = null;
          reject(new Error("d3.js を読み込めませんでした"));
        };
        document.head.appendChild(s);
      });
    }
    return d3Loading;
  }

  // ```d3 のブロック: スクリプトを d3・el（描く場所の要素）・width（その幅）を引数として実行する。
  // await も使える。要素（SVG など）や d3 の選択を返すと el に加える
  const AsyncFunction = (async () => {}).constructor;
  async function renderD3(root) {
    const blocks = root.querySelectorAll(".yy-d3:not([data-rendered])");
    if (!blocks.length) return;
    for (const b of blocks) b.setAttribute("data-rendered", "");
    try {
      await loadD3();
    } catch (e) {
      for (const b of blocks) showD3Error(b, e);
      return;
    }
    for (const b of blocks) {
      const src = b.querySelector(".yy-d3-src");
      if (!src || !b.isConnected) continue;
      for (const old of b.querySelectorAll(".yy-d3-out, .yy-d3-error")) old.remove();
      const el = document.createElement("div");
      el.className = "yy-d3-out";
      b.appendChild(el);
      try {
        const run = new AsyncFunction("d3", "el", "width", src.textContent);
        let out = await run(window.d3, el, el.clientWidth || content().clientWidth);
        if (out && typeof out.node === "function") out = out.node();
        if (out instanceof Node && !el.contains(out)) el.appendChild(out);
      } catch (e) {
        showD3Error(b, e);
      }
    }
  }

  function showD3Error(block, e) {
    const pre = document.createElement("pre");
    pre.className = "yy-d3-error";
    pre.textContent = "d3: " + (e && e.message ? e.message : String(e));
    block.appendChild(pre);
  }

  async function render(root) {
    renderMath(root);
    await renderDiagrams(root);
    await renderD3(root);
    document.body.setAttribute("data-d3-done", "");
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
      await renderD3(content());
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
