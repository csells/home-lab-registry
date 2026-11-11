'use strict';

const ResourceGraph=(()=>{
 const views=new WeakMap();let engine;
 const compare=(a,b)=>a.id.localeCompare(b.id),physical=r=>r.kind==='machine';
 function build(input,matching){
  const all=new Map([...input].sort(compare).map(r=>[r.id,r])),parents=new Map(),invalid=new Set(),unknown=Symbol('unplaced');
  for(const r of all.values()){
   if(physical(r)){parents.set(r.id,null);continue}
   const path=new Set([r.id]);let p=all.get(r.machine_id);
   while(p&&!physical(p)&&!path.has(p.id)){path.add(p.id);p=all.get(p.machine_id)}
   if(!p||!physical(p))invalid.add(r.id);
   const immediate=all.get(r.machine_id);
   parents.set(r.id,invalid.has(r.id)?unknown:immediate?.kind!=='service'?immediate?.id:unknown);
  }
  const included=new Set();
  for(const r of all.values())if(!matching||matching.has(r.id)){
   let id=r.id;const seen=new Set();while(all.has(id)&&!seen.has(id)){included.add(id);seen.add(id);id=parents.get(id)}
  }
  const records=new Map([...all].filter(([id])=>included.has(id))),nodes=new Map();
  const graph={id:'root',children:[],edges:[],layoutOptions:{
   'elk.algorithm':'layered','elk.direction':'RIGHT','elk.edgeRouting':'ORTHOGONAL',
   'elk.hierarchyHandling':'INCLUDE_CHILDREN','elk.randomSeed':'42',
   'elk.spacing.nodeNode':'36','elk.spacing.edgeNode':'24','elk.layered.spacing.nodeNodeBetweenLayers':'70',
   'elk.layered.considerModelOrder.strategy':'NODES_AND_EDGES','elk.json.edgeCoords':'ROOT',
   'elk.padding':'[top=24,left=24,bottom=24,right=24]'
  }};
  let index=0;
  for(const r of records.values())nodes.set(r.id,{id:'n'+index++,recordId:r.id,width:280,height:84,children:[],edges:[],layoutOptions:{
   'elk.padding':'[top=88,left=28,bottom=28,right=28]',
   'elk.nodeSize.constraints':'MINIMUM_SIZE','elk.nodeSize.minimum':'(300,100)'
  }});
  const unplaced=[...records.values()].filter(r=>parents.get(r.id)===unknown).length;
  if(unplaced)nodes.set(unknown,{id:'unplaced',children:[],edges:[],layoutOptions:{'elk.padding':'[top=88,left=28,bottom=28,right=28]'}});
  for(const r of records.values()){
   const n=nodes.get(r.id),p=nodes.get(parents.get(r.id));(p?p.children:graph.children).push(n);
  }
  if(unplaced)graph.children.push(nodes.get(unknown));
  for(const n of nodes.values())if(!n.children.length){delete n.children;delete n.layoutOptions;n.width=280;n.height=n.recordId&&records.get(n.recordId).kind!=='service'?100:84}
  function ancestors(id){const path=[];while(nodes.has(id)){path.push(id);id=parents.get(id)}return path}
  const edges=[],missing=[];
  for(const r of records.values())for(const target of [...new Set(r.dependencies||[])].sort()){
   if(target===r.id)continue;
   const e={id:'e'+edges.length,source:r.id,target};
   if(!records.has(target)){missing.push(e);continue}
   const sourcePath=ancestors(r.id),targetPath=new Set(ancestors(target)),common=sourcePath.find(id=>targetPath.has(id));
   const container=nodes.get(common)||graph;
   (container.edges||(container.edges=[])).push({id:e.id,sources:[nodes.get(r.id).id],targets:[nodes.get(target).id]});edges.push(e);
  }
  return {graph,records,edges,missing,unplaced};
 }
 function element(tag,cls,text){const n=document.createElement(tag);if(cls)n.className=cls;if(text!==undefined)n.textContent=text;return n}
 function svgElement(tag,attributes={}){const n=document.createElementNS('http://www.w3.org/2000/svg',tag);for(const [k,v] of Object.entries(attributes))n.setAttribute(k,v);return n}
 function mount(root,records,options={}){
  const previous=views.get(root);if(previous)previous.dispose();
  const input=build(records,options.matching),signature=JSON.stringify(input.graph),same=previous?.signature===signature;
  const view={signature,dispose:()=>{}};views.set(root,view);
  let disposed=false,deadline;
  root.replaceChildren();root.classList.add('resource-graph');root.dataset.layoutReady='false';
  const status=element('p','graph-status','Arranging resources...');status.setAttribute('role','status');
  const viewport=element('div','graph-viewport'),svg=svgElement('svg',{'aria-label':'Resource containment and declared dependencies',role:'img'}),canvas=svgElement('g');svg.append(canvas);viewport.append(svg);
  const legend=element('p','graph-legend','Boxes contain their resources. Dashed arrows point from caller to declared dependency.');
  root.append(status,viewport,legend);
  function render(result){
   if(disposed)return;clearTimeout(deadline);view.layout=result;canvas.replaceChildren();
   const pad=18;svg.style.width=result.width+2*pad+'px';svg.style.height=result.height+2*pad+'px';canvas.setAttribute('transform',`translate(${pad} ${pad})`);
   const defs=svgElement('defs'),marker=svgElement('marker',{id:'resource-arrow',viewBox:'0 0 10 10',refX:9,refY:5,markerWidth:7,markerHeight:7,orient:'auto'});marker.append(svgElement('path',{d:'M 0 0 L 10 5 L 0 10 z',class:'graph-arrow'}));defs.append(marker);canvas.append(defs);
   const backgrounds=svgElement('g'),routes=svgElement('g'),headers=svgElement('g');canvas.append(backgrounds,routes,headers);
   function visit(n,ox=0,oy=0){
    const x=ox+(n.x||0),y=oy+(n.y||0),r=input.records.get(n.recordId),group=!!n.children?.length;
    if(n.id!=='root'){
     const kind=r?.kind||'unplaced',rect=svgElement('rect',{x,y,width:n.width,height:n.height,rx:10,class:'resource-boundary resource-'+kind});rect.dataset.resourceBox=r?.id||'unplaced';backgrounds.append(rect);
     const foreign=svgElement('foreignObject',{x:x+8,y:y+8,width:n.width-16,height:group?64:n.height-16}),b=element('div','resource-node resource-'+kind);
     if(r){b.dataset.resourceId=r.id;b.title=r.name}
     b.append(element('span','resource-kind',kind==='machine'?'Physical machine':kind==='virtual-machine'?'Virtual machine':kind==='service'?'Application service':kind==='unplaced'?'Placement unknown':'Container'),element('strong','resource-label',r?.name||'Unplaced resources'));
     if(r)b.append(element('span','resource-state',r.status||'Unknown'));foreign.append(b);headers.append(foreign);
    }
    for(const child of n.children||[])visit(child,x,y);
    for(const e of n.edges||[])for(const section of e.sections||[]){
     const points=[section.startPoint,...section.bendPoints||[],section.endPoint],edge=input.edges.find(edge=>edge.id===e.id),path=svgElement('path',{d:points.map((p,i)=>(i?'L':'M')+p.x+' '+p.y).join(' '),class:'resource-edge','marker-end':'url(#resource-arrow)'});
     const title=svgElement('title');title.textContent=input.records.get(edge.source).name+' → '+input.records.get(edge.target).name;path.append(title);routes.append(path);
    }
   }
   visit(result);status.textContent=`${input.records.size} resources · ${input.edges.length} declared connections`+(input.unplaced?` · ${input.unplaced} unplaced`:'')+(input.missing.length?` · ${input.missing.length} targets outside this view`:'');
   root.dataset.layoutReady='true';
  }
  view.dispose=()=>{disposed=true;clearTimeout(deadline)};
  if(!input.records.size){status.textContent='No resources match these filters.';viewport.hidden=true;root.dataset.layoutReady='true';return view.dispose}
  try{
   if(same&&previous.layout)render(previous.layout);
   else{deadline=setTimeout(()=>{if(!disposed){status.textContent='Resource layout did not finish. Reload to retry.';status.setAttribute('role','alert')}},30000);engine=engine||new ELK({workerUrl:new URL('vendor/elk-worker.min.js',document.baseURI).href});engine.layout(input.graph).then(render).catch(error=>{clearTimeout(deadline);if(!disposed){status.textContent='Resource layout failed: '+error.message;status.setAttribute('role','alert')}})}
  }catch(error){status.textContent='Resource layout unavailable: '+error.message;status.setAttribute('role','alert')}
  return view.dispose;
 }
 return {build,mount};
})();
if(typeof module!=='undefined')module.exports=ResourceGraph;
