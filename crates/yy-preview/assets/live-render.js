// 数式・Mermaid と、編集ページから分離した d3 の描画。
(() => {
  'use strict';
  const cache=new Map();
  let serial=0, queue=Promise.resolve();
  const assets=new URL('.',document.currentScript.src).href;
  const frames=new Map();
  window.addEventListener('message',event=>{
    const entry=frames.get(event.source);
    if(!entry || event.origin!=='null') return;
    const {frame,node,source}=entry;
    if(!frame.isConnected) {frames.delete(event.source);return;}
    const data=event.data;
    if(data?.type==='yy-d3-ready') frame.contentWindow.postMessage({type:'yy-d3-render',source,base:document.baseURI},'*');
    if(data?.type==='yy-d3-size' && Number.isFinite(data.height)) frame.style.height=Math.min(10000,Math.max(40,data.height))+'px';
    if(data?.type==='yy-d3-result') {
      node.dataset.status=data.error?'error':'rendered';
      if(data.error) frame.title='d3: '+String(data.error).slice(0,300);
    }
    // 文書フォルダの読み取りは既存プレビューと同じ検証済みの資源経路を使う。
    if(data?.type==='yy-d3-fetch' && document.baseURI.startsWith('https://yy-doc.local/')) {
      let url;try{url=new URL(data.url);}catch{return;}
      if(url.origin!=='https://yy-doc.local') return;
      (async()=>{
        try {
          const response=await fetch(url.href),body=await response.arrayBuffer();
          if(frame.isConnected) frame.contentWindow.postMessage({type:'yy-d3-file',id:data.id,body,status:response.status,statusText:response.statusText,headers:[...response.headers]},'*',[body]);
        } catch(error) {
          if(frame.isConnected) frame.contentWindow.postMessage({type:'yy-d3-file',id:data.id,error:String(error)},'*');
        }
      })();
    }
  });
  function renderD3(root) {
    for(const [key,{frame}] of frames) if(!frame.isConnected) frames.delete(key);
    for(const node of root.querySelectorAll('.yy-d3:not([data-rendered])')) {
      const source=node.querySelector('.yy-d3-src')?.textContent;
      if(source===undefined) continue;
      node.dataset.rendered='';
      const edit=document.createElement('button');edit.type='button';edit.className='live-d3-edit';edit.textContent='d3 コードを編集';node.appendChild(edit);
      const frame=document.createElement('iframe');
      frame.className='live-d3-frame';frame.title='d3 の描画';
      frame.setAttribute('sandbox','allow-scripts');
      frame.src=assets+'live-d3.html';node.appendChild(frame);
      frames.set(frame.contentWindow,{frame,node,source});
    }
  }
  if(window.mermaid) mermaid.initialize({startOnLoad:false,securityLevel:'strict',theme:'default',suppressErrorRendering:true});
  window.yyLiveRender = root => {
    renderD3(root);
    if(window.katex) for(const node of root.querySelectorAll('.math:not([data-rendered])')) {
      const source=node.textContent;
      node.dataset.rendered='';
      try { katex.render(source,node,{displayMode:node.classList.contains('math-display'),throwOnError:false,trust:false}); }
      catch(error) {node.textContent=source;node.title=String(error);node.classList.add('render-error');}
    }
    if(window.mermaid) for(const node of root.querySelectorAll('pre.mermaid:not([data-rendered])')) {
      const source=node.textContent;
      node.dataset.rendered='';
      queue=queue.then(async()=>{
        if(!node.isConnected) return;
        const id='yy-live-mermaid-'+(++serial);
        try {
          let svg=cache.get(source);
          if(!svg) {svg=(await mermaid.render(id,source)).svg;if(cache.size>=64)cache.clear();cache.set(source,svg);}
          if(node.isConnected) {node.innerHTML=svg;node.classList.add('diagram');}
        } catch(error) {
          if(node.isConnected) {node.textContent=source+'\n\nMermaid: '+String(error.message||error).slice(0,300);node.classList.add('render-error');}
        } finally {
          for(const temp of [document.getElementById('d'+id),document.getElementById(id)]) if(temp && !node.contains(temp)) temp.remove();
        }
      }).catch(()=>{});
    }
  };
})();
