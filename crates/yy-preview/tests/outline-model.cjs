const {test}=require('node:test');
const assert=require('node:assert/strict');
const M=require('../assets/outline-model.js');
const node=(id,depth,text=id)=>({id,depth,text,bookmark:false,collapsed:false,refs:[]});
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
