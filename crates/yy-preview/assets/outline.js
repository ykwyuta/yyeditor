(() => {
  'use strict';
  const M=globalThis.YYOutline, bridge=window.chrome?.webview;
  const tree=document.getElementById('tree'), status=document.getElementById('status');
  const readonly=document.body.dataset.readonly==='true';
  let epoch=Number(document.body.dataset.epoch), seq=0, source='', doc=null, selected=null, zoom=null;
  let composing=false, pending=[], loaded=false;
  const byId=id=>doc?.nodes.find(n=>n.id===id);
  const index=()=>doc.nodes.findIndex(n=>n.id===selected);
  function send(action='edit') {
    if(composing) { pending.push(action); return; }
    bridge?.postMessage([epoch,++seq,action,0,0,source].join('\n'));
  }
  function button(label, action, parent) {
    const b=document.createElement('button'); b.type='button'; b.textContent=label;
    b.onclick=action; parent.append(b); return b;
  }
  function load(text) {
    source=text;
    try { doc=M.parse(source); selected=doc.nodes[0]?.id ?? null; render(); }
    catch(error) { doc=null; tree.textContent='アウトラインを読み込めません。'; status.textContent=error.message; }
    loaded=true;
    document.querySelectorAll('[data-op]').forEach(b=>b.disabled=readonly || !doc);
    send('ready');
  }
  function change(action, mutation, redraw=true) {
    if(readonly || !doc || composing) return;
    const before=M.serialize(doc), oldSelected=selected;
    mutation();
    const next=M.serialize(doc);
    if(new TextEncoder().encode(next).length>1048576) {
      doc=M.parse(before); selected=oldSelected;
      status.textContent='1 MiB の上限を超えるため変更できません。'; render(); return;
    }
    source=next; send(action);
    if(redraw) render(true);
    else {
      renderLinks();
      for(const link of tree.querySelectorAll('[data-target]')) {
        const target=byId(link.dataset.target);
        link.textContent='↗ '+(target?.text || (target?'(空の項目)':'参照先なし'));
      }
    }
  }
  function focus() {
    const row=[...tree.children].find(r=>r.dataset.id===selected);
    row?.querySelector('input')?.focus();
  }
  function navigate(id) {
    if(!byId(id)) return;
    zoom=id; selected=id; render(true);
  }
  function render(restore=false) {
    tree.replaceChildren();
    const crumbs=document.getElementById('crumbs'); crumbs.replaceChildren();
    button('全体',()=>{zoom=null;render(true);},crumbs);
    let start=zoom ? doc.nodes.findIndex(n=>n.id===zoom) : 0;
    if(start<0) { zoom=null; start=0; }
    if(zoom) {
      const ancestors=[]; let depth=doc.nodes[start].depth;
      for(let i=start-1;i>=0;i--) if(doc.nodes[i].depth<depth) {ancestors.push(doc.nodes[i]);depth=doc.nodes[i].depth;}
      for(const n of ancestors.reverse()) button(n.text || '(空の項目)',()=>navigate(n.id),crumbs);
      button(byId(zoom).text || '(空の項目)',()=>navigate(zoom),crumbs);
    }
    const stop=zoom ? M.end(doc,start) : doc.nodes.length, base=zoom ? doc.nodes[start].depth : 0;
    const lookup=new Map(doc.nodes.map(n=>[n.id,n]));
    let hiddenDepth=null;
    for(let i=start;i<stop;i++) {
      const n=doc.nodes[i];
      if(hiddenDepth!==null && n.depth>hiddenDepth) continue;
      hiddenDepth=null;
      const row=document.createElement('div'); row.className='row'+(n.id===selected?' active':''); row.dataset.id=n.id;
      row.style.marginLeft=((n.depth-base)*24)+'px';
      const children=i+1<doc.nodes.length && doc.nodes[i+1].depth>n.depth;
      const fold=button(children?(n.collapsed?'▸':'▾'):'·',()=>{selected=n.id;change('outline',()=>n.collapsed=!n.collapsed);},row);
      fold.className='fold'; fold.setAttribute('aria-label',n.collapsed?'展開':'折りたたみ'); fold.disabled=readonly || !children;
      const bullet=button('●',()=>navigate(n.id),row); bullet.className='zoom'; bullet.title='この階層にズーム';
      const input=document.createElement('input'); input.value=n.text; input.readOnly=readonly;
      input.setAttribute('aria-label','階層 '+(n.depth+1)+' の項目');
      input.onfocus=()=>{selected=n.id; tree.querySelectorAll('.row').forEach(r=>r.classList.toggle('active',r===row)); renderLinks();};
      input.oninput=()=>{if(!composing) change('edit',()=>n.text=input.value,false);};
      input.oncompositionstart=()=>{composing=true;};
      input.oncompositionend=()=>{composing=false; change('edit',()=>n.text=input.value,false);const actions=pending;pending=[];for(const a of actions)send(a);};
      input.onkeydown=e=>{
        if(e.isComposing || composing) return;
        if(e.key==='Enter') {e.preventDefault();operate('add');}
        else if(e.key==='Tab') {e.preventDefault();operate(e.shiftKey?'outdent':'indent');}
        else if(e.altKey && ['ArrowUp','ArrowDown'].includes(e.key)) {e.preventDefault();operate(e.key==='ArrowUp'?'up':'down');}
        else if(['ArrowUp','ArrowDown'].includes(e.key)) {
          e.preventDefault(); const rows=[...tree.children], at=rows.indexOf(row)+(e.key==='ArrowUp'?-1:1);
          rows[at]?.querySelector('input')?.focus();
        }
      };
      row.append(input);
      if(n.bookmark) {const star=document.createElement('span');star.textContent='★';row.append(star);}
      const refs=document.createElement('span');refs.className='refs';
      for(const target of n.refs) {
        const dest=lookup.get(target);
        const link=button('↗ '+(dest?.text || (dest?'(空の項目)':'参照先なし')),()=>navigate(target),refs);
        link.dataset.target=target;
        if(!dest) link.className='missing';
        if(!readonly) button('×',()=>{selected=n.id;change('outline',()=>n.refs=n.refs.filter(r=>r!==target));},refs).setAttribute('aria-label','参照を解除');
      }
      row.append(refs);tree.append(row);
      if(n.collapsed && n.id!==zoom) hiddenDepth=n.depth;
    }
    if(!doc.nodes.length) {const empty=document.createElement('p');empty.className='empty';empty.textContent='「項目追加」で書き始めます。';tree.append(empty);}
    renderLinks();if(restore)focus();
  }
  function renderLinks() {
    for(const [container,filter] of [['bookmarks',n=>n.bookmark],['backlinks',n=>n.refs.includes(selected)]]) {
      const area=document.getElementById(container);area.replaceChildren();
      for(const n of doc.nodes.filter(filter)) button(n.text || '(空の項目)',()=>navigate(n.id),area);
    }
  }
  function reference() {
    const from=byId(selected);if(!from)return;
    const dialog=document.createElement('dialog');dialog.id='picker';
    const label=document.createElement('label');label.textContent='参照先 '; const select=document.createElement('select');
    for(const n of doc.nodes) if(n.id!==from.id && !from.refs.includes(n.id)) {
      const option=document.createElement('option');option.value=n.id;option.textContent=n.text || '(空の項目)';select.append(option);
    }
    label.append(select);dialog.append(label);
    button('追加',()=>{if(select.value)change('outline',()=>from.refs.push(select.value));dialog.close();},dialog).disabled=!select.options.length;
    button('キャンセル',()=>dialog.close(),dialog);dialog.onclose=()=>{dialog.remove();focus();};document.body.append(dialog);dialog.showModal();
  }
  function operate(op) {
    if(readonly || !doc || composing)return;
    if(op==='reference') {reference();return;}
    if(op==='delete' && index()>=0) {
      const count=M.end(doc,index())-index();
      if(!window.confirm('この項目と配下 '+(count-1)+' 件を削除しますか？'))return;
    }
    change('outline',()=>{
      const at=index();
      if(op==='add') { selected=M.add(doc,at); if(zoom && at===doc.nodes.findIndex(n=>n.id===zoom)) {const n=byId(selected);n.depth++;} }
      else if(at>=0 && op==='bookmark') doc.nodes[at].bookmark=!doc.nodes[at].bookmark;
      else if(at>=0 && op==='delete') {M.remove(doc,at);if(!byId(zoom))zoom=null;selected=doc.nodes[Math.min(at,doc.nodes.length-1)]?.id ?? null;}
      else if(at>=0) {M.move(doc,at,op);if(zoom && selected!==zoom && !doc.nodes.slice(doc.nodes.findIndex(n=>n.id===zoom),M.end(doc,doc.nodes.findIndex(n=>n.id===zoom))).some(n=>n.id===selected))zoom=null;}
    });
  }
  document.querySelectorAll('[data-op]').forEach(b=>b.onclick=()=>operate(b.dataset.op));
  document.querySelectorAll('[data-command]').forEach(b=>b.onclick=()=>send(b.dataset.command));
  document.addEventListener('keydown',e=>{
    if(!(e.ctrlKey || e.metaKey) || e.isComposing)return;
    const command=({s:e.shiftKey?'saveas':'save',z:e.shiftKey?'redo':'undo',y:'redo',o:'open',n:'new',w:'close'})[e.key.toLowerCase()];
    if(command) {e.preventDefault();send(command);}
  });
  if(bridge) {
    bridge.addEventListener('message',event=>{
      const data=event.data;
      if(data.epoch===epoch && typeof data.initial==='string' && !loaded) load(data.initial);
      if(data.flush===epoch) {
        // Blur commits an active IME composition; compositionend drains pending flushes.
        if(composing) {pending.push('flush:'+epoch);document.activeElement?.blur();}
        else send('flush:'+epoch);
      }
    });
    send('load');
  } else load(document.getElementById('source').textContent);
})();
