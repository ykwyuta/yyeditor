// このページは allow-scripts のみの sandbox 内。親の原文・編集ブリッジにはアクセスしない。
(() => {
  'use strict';
  const output=document.getElementById('output');
  const notify=data=>parent.postMessage(data,'https://yy-preview.local');
  const resize=()=>notify({type:'yy-d3-size',height:Math.ceil(output.getBoundingClientRect().height)});
  new ResizeObserver(resize).observe(output);
  const requests=new Map(), nativeFetch=window.fetch.bind(window);
  let requestId=0;
  window.fetch=(input,options)=>{
    const url=new URL(input instanceof Request?input.url:String(input),document.baseURI);
    const method=options?.method || (input instanceof Request?input.method:'GET');
    if(url.origin!=='https://yy-doc.local' || method!=='GET') return nativeFetch(input,options);
    return new Promise((resolve,reject)=>{
      const id=++requestId;
      const timeout=setTimeout(()=>{requests.delete(id);reject(new Error('文書データの読み込みがタイムアウトしました'));},30000);
      requests.set(id,{resolve,reject,timeout});notify({type:'yy-d3-fetch',id,url:url.href});
    });
  };
  let started=false;
  window.addEventListener('message',async event=>{
    if(event.source!==parent || event.origin!=='https://yy-preview.local') return;
    if(event.data?.type==='yy-d3-file') {
      const entry=requests.get(event.data.id);if(!entry)return;
      requests.delete(event.data.id);clearTimeout(entry.timeout);
      if(event.data.error) entry.reject(new Error(event.data.error));
      else entry.resolve(new Response(event.data.body,{status:event.data.status,statusText:event.data.statusText,headers:event.data.headers}));
      return;
    }
    if(started || event.data?.type!=='yy-d3-render') return;
    started=true;
    try {
      const base=document.createElement('base');base.href=event.data.base;document.head.appendChild(base);
      const AsyncFunction=(async()=>{}).constructor;
      const run=new AsyncFunction('d3','el','width',event.data.source);
      let result=await run(window.d3,output,output.clientWidth);
      if(result && typeof result.node==='function') result=result.node();
      if(result instanceof Node && !output.contains(result)) output.appendChild(result);
      notify({type:'yy-d3-result'});
    } catch(error) {
      const message=String(error.message||error).slice(0,1000);
      const pre=document.createElement('pre');pre.textContent='d3: '+message;output.appendChild(pre);
      notify({type:'yy-d3-result',error:message});
    }
    resize();
  });
  notify({type:'yy-d3-ready'});
})();
