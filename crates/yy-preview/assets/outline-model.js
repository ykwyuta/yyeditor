/* Flat preorder storage keeps depth independent of the JavaScript call stack. */
((scope) => {
  'use strict';
  const fresh = () => ({format:'yyoutline', version:1, nodes:[]});
  const id = () => 'n-' + globalThis.crypto.randomUUID();
  function parse(source) {
    if (!source.trim()) return fresh();
    const doc = JSON.parse(source);
    if (doc.format !== 'yyoutline' || doc.version !== 1 || !Array.isArray(doc.nodes)) throw Error('yyoutline version 1 のファイルを開いてください。');
    const ids = new Set();
    let previous = -1;
    for (const n of doc.nodes) {
      if (!n || typeof n.id !== 'string' || !n.id || ids.has(n.id) ||
          !Number.isSafeInteger(n.depth) || n.depth < 0 || n.depth > previous + 1 ||
          typeof n.text !== 'string' || typeof n.bookmark !== 'boolean' ||
          typeof n.collapsed !== 'boolean' || !Array.isArray(n.refs) ||
          n.refs.some(r => typeof r !== 'string')) throw Error('不正な項目・ID・階層が含まれています。通常編集で確認してください。');
      ids.add(n.id); previous = n.depth;
    }
    return doc;
  }
  const serialize = doc => JSON.stringify(doc, null, 2) + '\n';
  function end(doc, at) {
    let next = at + 1;
    while (next < doc.nodes.length && doc.nodes[next].depth > doc.nodes[at].depth) next++;
    return next;
  }
  function add(doc, at, text='') {
    const node = {id:id(), depth:at < 0 ? 0 : doc.nodes[at].depth, text, bookmark:false, collapsed:false, refs:[]};
    doc.nodes.splice(at < 0 ? doc.nodes.length : end(doc,at), 0, node);
    return node.id;
  }
  function move(doc, at, operation) {
    const nodes=doc.nodes, stop=end(doc,at), depth=nodes[at].depth;
    if (operation==='indent') {
      if (!at || nodes[at-1].depth < depth) return false;
      for(let i=at;i<stop;i++) nodes[i].depth++;
      // The preceding sibling is now the parent. Reveal the moved branch.
      let parent=at-1;
      while(nodes[parent].depth>depth) parent--;
      nodes[parent].collapsed=false;
    } else if (operation==='outdent') {
      if (!depth) return false;
      // Move after all remaining siblings so they do not become children.
      let parent=at-1;
      while(nodes[parent].depth>=depth) parent--;
      const destination=end(doc,parent), branch=nodes.splice(at,stop-at);
      for(const n of branch) n.depth--;
      const insert=destination-branch.length;
      doc.nodes=nodes.slice(0,insert).concat(branch,nodes.slice(insert));
    } else if (operation==='up') {
      if (!at || nodes[at-1].depth < depth) return false;
      let previous=at-1;
      while(nodes[previous].depth>depth) previous--;
      const branch=nodes.splice(at,stop-at);
      // Avoid argument-count limits for large branches.
      doc.nodes=nodes.slice(0,previous).concat(branch,nodes.slice(previous));
    } else if (operation==='down') {
      if(stop===nodes.length || nodes[stop].depth!==depth) return false;
      const destination=end(doc,stop), branch=nodes.slice(at,stop);
      doc.nodes=nodes.slice(0,at).concat(nodes.slice(stop,destination),branch,nodes.slice(destination));
    } else return false;
    return true;
  }
  function remove(doc, at) {
    doc.nodes.splice(at,end(doc,at)-at);
    // Keep references as explicit missing links; never silently retarget them.
  }
  scope.YYOutline={fresh,parse,serialize,end,add,move,remove};
  if(typeof module!=='undefined') module.exports=scope.YYOutline;
})(globalThis);
