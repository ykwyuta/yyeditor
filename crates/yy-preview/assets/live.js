/* 原文を唯一の保存形式にする。HTML から Markdown を再生成しない。 */
(() => {
  'use strict';
  const root = document.getElementById('blocks');
  let epoch = Number(document.body.dataset.epoch);
  const readonly = document.body.dataset.readonly === 'true';
  let source = document.getElementById('source').value;
  // textarea.value は CRLF を LF に変換するため、初期原文は textContent で読む。
  source = document.getElementById('source').textContent;
  let seq = 0, active = null, input = null, composing = false, rendered = null, deferred = null;
  let tablePending = false, tableContext = null;
  const bridge = window.chrome && window.chrome.webview;
  if(readonly) {
    document.querySelectorAll('[data-format]').forEach(button => { button.disabled=true; });
    document.getElementById('hint').textContent='読み取り専用 · 通常編集と同じ原文を表示しています';
  }
  function selection() {
    const start = active ? Number(active.dataset.start) : 0;
    const eol = source.includes('\r\n') ? '\r\n' : '\n';
    const offset = position => input ? input.value.slice(0,position).replace(/\n/g,eol).length : 0;
    return [start + offset(input ? input.selectionStart : 0), start + offset(input ? input.selectionEnd : 0)];
  }
  function send(action = 'edit') {
    if(tablePending) { if(action!=='edit' && action!=='select') deferred=action; return; }
    if (composing) { if (action !== 'edit' && action !== 'select') deferred = action; return; }
    const [a,b] = selection();
    if (bridge) bridge.postMessage([epoch, ++seq, action, a, b, source].join('\n'));
  }
  function size() {
    if (!input) return;
    input.classList.toggle('heading', /^\s*#{1,6}\s/.test(input.value));
    input.style.height = 'auto'; input.style.height = input.scrollHeight + 'px';
  }
  function update() {
    if (!input || readonly) return;
    const start = Number(active.dataset.start), end = Number(active.dataset.end);
    // textarea の改行は LF。既存の CRLF は編集対象の中も維持する。
    const eol = source.includes('\r\n') ? '\r\n' : '\n';
    const text = input.value.replace(/\r?\n/g, eol);
    const delta = text.length - (end-start);
    const next = source.slice(0,start) + text + source.slice(end);
    if (new TextEncoder().encode(next).length > 1048576) {
      input.value = source.slice(start,end); size();
      document.getElementById('hint').textContent = 'ライブ編集は 1 MiB までです。通常編集に戻って続けてください。';
      return;
    }
    source = next; rendered = null;
    active.dataset.end = start + text.length;
    let following = false;
    for (const node of root.children) {
      if (node === active) { following = true; continue; }
      if (following) { node.dataset.start = Number(node.dataset.start)+delta; node.dataset.end = Number(node.dataset.end)+delta; }
    }
    size(); send();
  }
  function applyRender() {
    if (rendered !== null) { root.innerHTML = rendered; rendered = null; }
    else if (active) {
      const fallback=document.createElement('pre');
      fallback.textContent=source.slice(Number(active.dataset.start),Number(active.dataset.end));
      active.replaceChildren(fallback); active.classList.remove('active');
    }
    active = input = null;
  }
  function activate(offset, endAt = false, all = false) {
    if (composing || tablePending) return;
    applyRender();
    let block = [...root.children].find(node => Number(node.dataset.end) > offset) || root.lastElementChild;
    if (all) {
      root.innerHTML = '<section class="block" data-start="0"></section>';
      block = root.firstElementChild; block.dataset.end = source.length;
    }
    if (!block) return;
    active = block; block.classList.add('active');
    input = document.createElement('textarea'); input.className = 'editor';
    input.setAttribute('aria-label', all ? '文書全体の Markdown 原文' : 'この段落の Markdown 原文');
    input.spellcheck = false; input.readOnly = readonly;
    const start = Number(block.dataset.start);
    input.value = source.slice(start, Number(block.dataset.end));
    block.replaceChildren(input); size(); input.focus();
    // DOM textarea の位置は LF で数える。
    const at = endAt ? input.value.length : source.slice(start, Math.max(start,offset)).replace(/\r\n/g,'\n').length;
    input.setSelectionRange(at,at);
    input.addEventListener('input', update);
    input.addEventListener('compositionstart', () => { composing = true; });
    input.addEventListener('compositionend', () => { composing = false; update(); if (deferred) { const action=deferred; deferred=null; send(action); } });
    input.addEventListener('select', () => send('select'));
    input.addEventListener('click', () => send('select'));
    input.addEventListener('keyup', () => send('select'));
    send('select');
  }
  function tableControls(context) {
    tableContext=context;
    for(const button of document.querySelectorAll('[data-table]')) {
      const op=button.dataset.table;
      button.disabled=readonly || tablePending || !context
        || (op==='delete-column' && context.columns===1)
        || ((op==='row-before' || op==='delete-row') && context.row===0);
    }
    document.getElementById('table-position').textContent=context
      ? `表: ${context.row===0?'見出し':context.row+'行'}・${context.column+1}列 (${context.rows-1}行 / ${context.columns}列)`
      : '表のセルを選択';
  }
  function tableEdit(operation) {
    if(readonly || composing || tablePending || !tableContext) return;
    send('table:'+operation);
    tablePending=true;
    if(input) input.readOnly=true;
    tableControls(tableContext);
  }
  // 初期 HTML の改行正規化を避け、ホストから送られた JSON 原文で初期化する。
  function format(kind) {
    if (readonly || composing || tablePending) return;
    if (!input) activate(0);
    const a=input.selectionStart,b=input.selectionEnd, text=input.value, selected=text.slice(a,b);
    const wrap = {bold:['**','**'],italic:['*','*'],strike:['~~','~~'],code:['`','`'],link:['[','](https://)']};
    let insert, ca, cb;
    if (wrap[kind]) { const [l,r]=wrap[kind]; insert=l+selected+r; ca=a+l.length; cb=ca+selected.length; }
    else {
      const line=a===0 ? 0 : text.lastIndexOf('\n',a-1)+1;
      const prefix={heading:'# ',list:'- ',quote:'> '}[kind];
      if (prefix) { input.setSelectionRange(line,line); input.setRangeText(prefix,line,line,'end'); update(); return; }
      insert=kind==='table' ? '| 見出し | 見出し |\n| --- | --- |\n| 内容 | 内容 |\n' : '```\n'+selected+'\n```\n';
      ca=cb=a+(kind==='fence'?4:insert.length);
    }
    input.setRangeText(insert,a,b,'end'); input.setSelectionRange(ca,cb); update(); input.focus();
  }
  root.addEventListener('click', event => {
    if(tablePending) { event.preventDefault(); return; }
    if (event.target === input) return;
    event.preventDefault();
    const block=event.target.closest('.block');
    const cell=event.target.closest('td[data-source],th[data-source]');
    if (block) activate(cell ? Number(cell.dataset.source) : Number(block.dataset.start));
    else activate(source.length, true);
  });
  document.querySelector('header').addEventListener('mousedown', event => { if(event.target.closest('button')) event.preventDefault(); });
  document.querySelector('header').addEventListener('click', event => {
    const button=event.target.closest('button'); if(!button) return;
    if (button.dataset.table) tableEdit(button.dataset.table);
    else if (button.dataset.format) format(button.dataset.format);
    else if (button.dataset.action==='all') activate(0,false,true);
    else if (button.dataset.command) send(button.dataset.command);
  });
  document.addEventListener('keydown', event => {
    if (event.isComposing || composing) return;
    const key=event.key.toLowerCase();
    if (event.ctrlKey || event.metaKey) {
      if (key==='b'||key==='i') { event.preventDefault(); format(key==='b'?'bold':'italic'); return; }
      const action={s:event.shiftKey?'saveas':'save',z:event.shiftKey?'redo':'undo',y:'redo',o:'open',n:'new',w:'close',tab:event.shiftKey?'prev':'next',f:event.shiftKey?'grep':'find',h:'replace',g:'goto',v:event.shiftKey?'preview':undefined}[key];
      if (action) { event.preventDefault(); send(action); return; }
    }
    if (event.key==='F3') { event.preventDefault(); send(event.shiftKey?'findprev':'findnext'); return; }
    if (event.key==='F1') { event.preventDefault(); send('help'); return; }
    if (!input || readonly || tablePending) return;
    if (event.key==='Escape') { event.preventDefault(); send('source'); return; }
    if (event.key==='Tab') { event.preventDefault(); input.setRangeText('    ',input.selectionStart,input.selectionEnd,'end'); update(); }
    if (event.key==='Enter' && !event.shiftKey) {
      const before=input.value.slice(0,input.selectionStart), line=before.slice(before.lastIndexOf('\n')+1);
      const match=line.match(/^(\s*)([-+*]|\d+[.)])\s+(?:\[[ xX]\]\s+)?/);
      if (match) {
        event.preventDefault();
        if (!line.slice(match[0].length).trim()) input.setRangeText('',before.length-line.length,input.selectionEnd,'end');
        else { const marker=/^\d/.test(match[2]) ? String(parseInt(match[2],10)+1)+match[2].slice(-1) : match[2]; const task=/\[[ xX]\]/.test(match[0])?'[ ] ':''; input.setRangeText('\n'+match[1]+marker+' '+task,input.selectionStart,input.selectionEnd,'end'); }
        update();
      }
    }
    if (event.key==='ArrowUp' && input.selectionStart===0) { event.preventDefault(); activate(Math.max(0,Number(active.dataset.start)-1),true); }
    if (event.key==='ArrowDown' && input.selectionEnd===input.value.length) { event.preventDefault(); activate(Number(active.dataset.end)); }
  });
  if (bridge) bridge.addEventListener('message', event => {
    const data=event.data;
    if(data.rebase===epoch) {
      epoch=data.epoch; document.body.dataset.epoch=epoch;
      seq=0; tablePending=false; source=data.initial; rendered=data.html;
      activate(data.caret); tableControls(data.table); send('ready');
      if(deferred) {const action=deferred;deferred=null;send(action);}
      return;
    }
    if (data.epoch !== undefined && data.epoch !== epoch) return;
    if (data.html !== undefined && data.seq===seq) { rendered=data.html; if(data.table!==undefined) tableControls(data.table); }
    if(data.tableError!==undefined) {
      tablePending=false; if(input) input.readOnly=readonly;
      tableControls(tableContext); document.getElementById('hint').textContent=data.tableError;
      if(deferred) {const action=deferred;deferred=null;send(action);}
    }
    if (data.flush !== undefined) { if(composing && input) input.blur(); send('flush:'+data.flush); }
    if (data.initial !== undefined) { source=data.initial; activate(Number(document.body.dataset.caret)); send('ready'); }
  });
  // ホストは ready 通知後に原文を JSON で送り返す（CRLF と末尾改行を維持）。
  if(bridge) bridge.postMessage([epoch,0,'load',0,0,''].join('\n'));
})();
