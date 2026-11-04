const {test}=require('node:test');
const assert=require('node:assert/strict');
const M=require('../assets/outline-model.js');
const node=(id,depth,text=id)=>({id,depth,text,bookmark:false,collapsed:false,refs:[]});

test('paste splits LF/CRLF/CR into siblings and preserves IDs, children and references',()=>{
  for(const newline of ['\n','\r\n','\r']) {
    const doc=M.fresh();doc.nodes=[node('root',0,'前😀選択後'),node('child',1),node('next',0)];
    doc.nodes[0].bookmark=true;doc.nodes[0].collapsed=true;doc.nodes[2].refs=['root'];
    const result=M.paste(doc,0,['一','二','','三',''].join(newline),3,5,'lines');
    assert.deepEqual(doc.nodes.map(n=>[n.text,n.depth]),[['前😀一',0],['child',1],['二',0],['',0],['三',0],['後',0],['next',0]]);
    assert.equal(doc.nodes[0].id,'root');assert.equal(doc.nodes[0].bookmark,true);assert.equal(doc.nodes[0].collapsed,true);
    assert.deepEqual(doc.nodes.at(-1).refs,['root']);
    assert.equal(result.id,doc.nodes[5].id);assert.equal(result.caret,0);assert.equal(result.added,4);
    assert.equal(new Set(doc.nodes.map(n=>n.id)).size,doc.nodes.length);
    assert.deepEqual(M.parse(M.serialize(doc)),doc);
  }
});

test('single-line paste replaces only selected text and normalizes newlines to spaces',()=>{
  const doc=M.fresh();doc.nodes=[node('a',0,'前😀選択後'),node('child',1)];
  const result=M.paste(doc,0,'日本語\r\n次\r\n\n末尾',3,5,'single');
  assert.equal(doc.nodes[0].text,'前😀日本語 次  末尾後');
  assert.equal(doc.nodes.length,2);assert.equal(result.id,'a');
  assert.equal(result.caret,'前😀日本語 次  末尾'.length);assert.equal(result.added,0);
  assert.throws(()=>M.paste(doc,0,'text',0,0,'unknown'));
});
test('Markdown uses six heading levels then nested lists and includes collapsed branches',()=>{
  const doc=M.fresh();doc.nodes=Array.from({length:9},(_,i)=>node('n'+i,i,'項目 '+i));
  doc.nodes[0].collapsed=true;doc.nodes.push(node('last',0,'終わり <script> & *強調*\n次行'));
  const before=M.serialize(doc);
  assert.equal(M.markdown(doc),'# 項目 0\n\n## 項目 1\n\n### 項目 2\n\n#### 項目 3\n\n##### 項目 4\n\n###### 項目 5\n- 項目 6\n    - 項目 7\n        - 項目 8\n\n# 終わり \\<script\\> \\& \\*強調\\* 次行\n');
  assert.equal(M.serialize(doc),before);assert.equal(M.markdown(M.fresh()),'');
  doc.nodes=Array.from({length:2000},(_,i)=>node('n'+i,i));
  assert.throws(()=>M.markdown(doc),/1 MiB/);
});
test('numbers restart for each parent, survive collapsed branches and persist style',()=>{
  const doc=M.fresh();doc.nodes=[node('a',0),node('b',1),node('c',2),node('d',1),node('e',0),node('f',1)];
  doc.nodes[0].collapsed=true;doc.listStyle='numbered';
  assert.deepEqual(M.numbers(doc),[1,1,1,2,2,1]);
  assert.deepEqual(M.parse(M.serialize(doc)),doc);
  M.move(doc,4,'up');assert.deepEqual(M.numbers(doc),[1,1,2,1,1,2]);
  assert.throws(()=>M.parse(JSON.stringify({...doc,listStyle:'invalid'})));
});
test('only complete HTTP URLs become links, with safe punctuation and balanced parentheses',()=>{
  assert.deepEqual(M.urls('参照「https://example.com/a?q=1&b=2」。 http://example.org/test. https://example.com/a_(b) (https://example.net/x) javascript:alert(1) https://'),['https://example.com/a?q=1&b=2','http://example.org/test','https://example.com/a_(b)','https://example.net/x']);
  assert.deepEqual(M.urls('<script>alert(1)</script> file:///C:/secret data:text/html,test'),[]);
});
test('deep outlines roundtrip without recursion and preserve Japanese, IDs and links',()=>{
  const doc=M.fresh();
  for(let i=0;i<12000;i++)doc.nodes.push(node('n'+i,i,'日本語😀 '+i));
  doc.nodes[11999].bookmark=true;doc.nodes[0].refs=['n11999'];
  assert.deepEqual(M.parse(M.serialize(doc)),doc);
  assert.equal(M.end(doc,0),12000);
});
test('indent and outdent move entire branches without adopting following siblings',()=>{
  const doc=M.fresh();doc.nodes=[node('a',0),node('b',1),node('c',2),node('d',1),node('e',0)];
  doc.nodes[1].bookmark=true;doc.nodes[4].refs=['b'];
  assert.equal(M.move(doc,1,'outdent'),true);
  assert.deepEqual(doc.nodes.map(n=>[n.id,n.depth]),[['a',0],['d',1],['b',0],['c',1],['e',0]]);
  assert.equal(M.move(doc,2,'indent'),true);
  assert.deepEqual(doc.nodes.map(n=>[n.id,n.depth]),[['a',0],['d',1],['b',1],['c',2],['e',0]]);
  assert.equal(doc.nodes[2].bookmark,true);assert.deepEqual(doc.nodes[4].refs,['b']);
  M.parse(M.serialize(doc));
});
test('reorder siblings with descendants and keep references stable across rename and deletion',()=>{
  const doc=M.fresh();doc.nodes=[node('a',0),node('b',1),node('c',0),node('d',1),node('e',0)];
  doc.nodes[4].refs=['a'];
  M.move(doc,0,'down');assert.deepEqual(doc.nodes.map(n=>n.id),['c','d','a','b','e']);
  M.move(doc,2,'up');assert.deepEqual(doc.nodes.map(n=>n.id),['a','b','c','d','e']);
  doc.nodes[0].text='renamed';assert.equal(doc.nodes[4].refs[0],'a');
  M.remove(doc,0);assert.deepEqual(doc.nodes.map(n=>n.id),['c','d','e']);assert.equal(doc.nodes[2].refs[0],'a');
  assert.deepEqual(M.parse(M.serialize(doc)),doc);
});
test('reject malformed and duplicate IDs, unsupported versions and depth jumps',()=>{
  for(const nodes of [[node('a',1)],[node('a',0),node('a',0)],[node('a',0),node('b',2)]]) {
    assert.throws(()=>M.parse(JSON.stringify({format:'yyoutline',version:1,nodes})));
  }
  assert.throws(()=>M.parse('{"format":"yyoutline","version":2,"nodes":[]}'));
  assert.throws(()=>M.parse('ordinary text'));
  assert.deepEqual(M.parse(''),M.fresh());
});
test('root boundaries, insertion and large branch moves',()=>{
  const doc=M.fresh();const first=M.add(doc,-1,'First');M.add(doc,0,'Second');
  assert.equal(M.move(doc,0,'indent'),false);assert.equal(M.move(doc,0,'outdent'),false);
  assert.equal(M.move(doc,0,'up'),false);assert.equal(M.move(doc,1,'down'),false);
  assert.equal(doc.nodes[0].id,first);
  doc.nodes=[node('root',0),node('branch',1)];
  for(let i=0;i<150000;i++)doc.nodes.push(node('x'+i,2));
  assert.equal(M.move(doc,1,'outdent'),true);
  assert.equal(doc.nodes[1].depth,0);assert.equal(doc.nodes.at(-1).depth,1);
});
