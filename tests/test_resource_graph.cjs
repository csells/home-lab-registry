const assert=require('node:assert/strict');
const {build}=require('../dashboard/resource-graph.js');
const ELK=require('../dashboard/vendor/elk-api.js');
const {Worker}=require('../dashboard/vendor/elk-worker.min.js');
const records=[
 {id:'host',name:'Host',kind:'machine'}, {id:'peer',name:'Peer Mac',kind:'machine',machine_id:'host'},
 {id:'a',name:'Office guest',kind:'container',machine_id:'host',dependencies:['host']},
 {id:'b',name:'GC guest',kind:'virtual-machine',machine_id:'host'},
 {id:'office',name:'Office',kind:'service',machine_id:'a',dependencies:['gc']},
 {id:'gc',name:'Gas City',kind:'service',machine_id:'b',dependencies:['office']},
 {id:'lost',name:'Unknown placement',kind:'container',machine_id:'missing'},
 {id:'cycle1',name:'Cycle one',kind:'container',machine_id:'cycle2'},
 {id:'cycle2',name:'Cycle two',kind:'container',machine_id:'cycle1'},
];
(async()=>{
 const input=build(records);const peer=input.graph.children.find(n=>n.recordId==='peer');assert(peer,'physical peer remains at root');
 assert.equal(input.unplaced,3);assert.equal(input.edges.length,3);
 const host=input.graph.children.find(n=>n.recordId==='host');assert.equal(host.edges.length,3,'edges belong to their common ancestor');
 const result=await new ELK({workerFactory:()=>new Worker()}).layout(input.graph);assert(result.width>0&&result.height>0);
 let edgeCount=0;const boxes=[];
 function walk(n,x=0,y=0){x+=n.x||0;y+=n.y||0;for(const child of n.children||[])walk(child,x,y);if(n.recordId)boxes.push({id:n.recordId,x,y,w:n.width,h:n.height});for(const edge of n.edges||[]){edgeCount++;assert(edge.sections?.length);for(const section of edge.sections){const points=[section.startPoint,...section.bendPoints||[],section.endPoint];for(let i=1;i<points.length;i++)assert(points[i].x===points[i-1].x||points[i].y===points[i-1].y,'orthogonal routing')}}}
 walk(result);assert.equal(edgeCount,3);assert.equal(boxes.length,records.length);
 assert.equal(JSON.stringify(build([...records].reverse()).graph),JSON.stringify(build(records).graph),'stable input ordering');
 const filtered=build(records,new Set(['office']));assert.equal(filtered.records.size,3,'ancestors retained, unrelated nodes excluded');assert.equal(filtered.missing.length,1);
 console.log('PASS graph builder and ELK: physical peers, containment, ancestor edges, cross-group cycle, invalid placement, filtering, stable input and orthogonal routes');
})().catch(e=>{console.error(e);process.exitCode=1});
