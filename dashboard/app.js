'use strict';
const $=id=>document.getElementById(id);
let data={machines:[],services:[]},view=location.hash==='#services'?'services':'machines',filter='all',busy=false,offset=0,copying=0;
const opened=new Set(),feedback=new Map(),statuses=['healthy','unhealthy','stale','unknown'];
const now=()=>Date.now()/1000+offset;
function el(tag,cls,text){const n=document.createElement(tag);if(cls)n.className=cls;if(text!==undefined)n.textContent=text;return n}
function age(t){if(!t)return 'No reading';const m=Math.max(0,Math.floor((now()-t)/60));if(m<1)return 'Just now';if(m<60)return m+'m ago';if(m<1440)return Math.floor(m/60)+'h ago';return Math.floor(m/1440)+'d ago'}
function bytes(n){return n==null?'Unknown':(n/1073741824).toFixed(1)+' GiB'}
function health(r){const h=r.health||{};return h.expires&&h.expires<now()?'stale':h.status||'unknown'}
function safeUrl(value,ssh=false){try{const u=new URL(value);return (ssh?['http:','https:','ssh:']:['http:','https:']).includes(u.protocol)&&!u.password?u:null}catch{return null}}
function link(label,url,cls='button primary'){const a=el('a',cls,label);a.href=url;a.target='_blank';a.rel='noopener noreferrer';return a}
function symbol(){const n=el('span','machine-symbol');n.setAttribute('aria-hidden','true');n.innerHTML='<svg viewBox="0 0 24 24"><rect x="4" y="3" width="16" height="7" rx="2"/><rect x="4" y="14" width="16" height="7" rx="2"/><path d="M8 6.5h.01M8 17.5h.01M13 6.5h4M13 17.5h4"/></svg>';return n}
function details(key,cls,label){const d=el('details',cls);d.open=opened.has(key);d.append(el('summary','',label));d.addEventListener('toggle',()=>{if(d.isConnected){if(d.open)opened.add(key);else opened.delete(key)}});return d}
function timeElement(t){const d=el('time','',age(t));if(t){d.dateTime=new Date(t*1000).toISOString();d.title=new Date(t*1000).toLocaleString()}return d}
function switchView(v){view=v;filter='all';$('search').value='';$('host').value='';history.replaceState(null,'','#'+v);render()}
function setFeedback(key,node,message){feedback.set(key,message);node.textContent=message.startsWith('Could not copy')?'Copy failed':message;node.parentElement.title=message;$('toast').textContent=message}
function copyPassword(r,a,status){const b=el('button','button secondary password-copy');b.type='button';for(const label of ['Copy password','Password copied','Copying…','Copy failed']){const size=el('span','copy-size',label);size.setAttribute('aria-hidden','true');b.append(size)}status.className='copy-label';const previous=feedback.get(r.id+':'+a.id)||'Copy password';status.textContent=previous.startsWith('Could not copy')?'Copy failed':previous;b.title=previous;b.append(status);b.setAttribute('aria-label','Copy '+r.host+' '+a.label+' password');
 b.onclick=async()=>{b.disabled=true;copying++;const key=r.id+':'+a.id;setFeedback(key,status,'Copying…');try{
  if(!navigator.clipboard||!window.ClipboardItem)throw Error('unsupported');
  const value=fetch('/api/copy-password',{method:'POST',headers:{'Content-Type':'application/json','X-Lab-Action':'copy-password'},body:JSON.stringify({machine:r.id,login:a.id}),cache:'no-store',signal:AbortSignal.timeout(15000)}).then(r=>{if(!r.ok)throw Error('failed');return r.text()}).then(text=>new Blob([text],{type:'text/plain'}));
  await navigator.clipboard.write([new ClipboardItem({'text/plain':value})]);setFeedback(key,status,'Password copied');
 }catch{setFeedback(key,status,'Could not copy. Check clipboard permission and retry.')}finally{b.disabled=false;copying--}};return b}
function accessPanel(r){const box=el('section','access-panel');box.setAttribute('aria-label',r.host+' login paths');const head=el('div','section-kicker','CONNECT');head.append(el('span','plain','Your ways into '+r.host));box.append(head);const grid=el('div','access-grid'),unavailable=[];
 for(const a of r.access||[]){const url=safeUrl(a.url,true);if(!url){unavailable.push(a);continue}const ssh=url.protocol==='ssh:';if(ssh&&!a.browser_terminal){unavailable.push({...a,note:'Browser terminal not configured.'});continue}if(ssh&&a.username)url.username=a.username;
  const row=el('section','access-row');row.dataset.login=a.id;const title=ssh?'SSH terminal':a.id==='kvm'?'Network KVM':a.id==='amt-off'?'Intel AMT · OS off':a.id==='amt'?'Intel AMT':a.label;
  const h=el('div','route-head');h.append(el('h4','',title),el('span','route-kind',ssh&&a.browser_terminal?'BROWSER':'LAN / VPN'));row.append(h,el('p','login-address',url.host));
  const credentials=el('div','credential-line'),user=el('div','username');user.append(el('span','username-label',ssh?'Connects as':'Username'),el('div','username-value'+(!a.username?' no-username':''),a.username||(a.username_required===false?'Not required':'Not recorded')));credentials.append(user);
  const status=el('div','copy-status',feedback.get(r.id+':'+a.id)||'');status.setAttribute('role','status');if(a.password_available&&!ssh)credentials.append(copyPassword(r,a,status));row.append(credentials);
  if(['kvm','amt','amt-off'].includes(a.id)){
   user.querySelector('.username-label').textContent='Device username';
   const consoleLogin=r.console_login,consoleRow=el('div','credential-line console-credentials'),consoleUser=el('div','username');
   consoleUser.append(el('span','username-label','Host console username'),el('div','username-value'+(!consoleLogin?.username?' no-username':''),consoleLogin?.username||'Not recorded'));consoleRow.append(consoleUser);
   if(consoleLogin?.password_available){const consoleStatus=el('span');consoleRow.append(copyPassword(r,{id:consoleLogin.credential_id,label:'host console'},consoleStatus))}
   else consoleRow.append(el('span','console-unavailable','Password not recorded'));
   row.append(consoleRow);
  }
  const actions=el('div','login-actions');
  if(ssh&&a.browser_terminal&&a.browser_terminal.startsWith('/terminal/')){const l=link('Open SSH terminal ↗',a.browser_terminal);l.setAttribute('aria-label','Open SSH terminal · '+r.host);actions.append(l)}else{const label=ssh?'Open SSH app':a.id==='proxmox'?'Open Proxmox':a.id==='kvm'?'Open KVM':'Open Intel AMT';const l=link(label+' ↗',url.href);l.setAttribute('aria-label',label+' · '+r.host+(a.id==='amt-off'?' · OS off':''));actions.append(l)}row.append(actions);
  const more=details(r.id+':route:'+a.id,'route-details','Login details');
  if(ssh){more.append(el('p','','This terminal runs in your browser and connects through the lab. You are logged in automatically; no SSH password or installed app is required.'));
  }else more.append(el('p','','This opens the device’s own login page. You need home-network or VPN access.'));
  if(a.note)more.append(el('p','',a.note));if(!ssh&&!a.password_available)more.append(el('p','',a.password_note||'No password stored for this login.'));row.append(more);grid.append(row);
 }
 if(grid.children.length)box.append(grid,el('p','access-hint','Browser SSH works from anywhere. Device login pages marked LAN / VPN require access to your home network.'));
 else box.append(el('p','empty-access','No verified login paths yet. Access details need to be registered before this machine can be opened.'));
 if(unavailable.length){const d=details(r.id+':unavailable','unavailable-paths',unavailable.length+' unavailable '+(unavailable.length===1?'path':'paths'));for(const a of unavailable)d.append(el('p','',a.label+' — '+(a.note||'Not configured.')));box.append(d)}return box;
}
function meter(label,used,total,pct){const d=el('div','metric');pct=pct??(total?used/total*100:null);const head=el('div','metric-head');head.append(el('span','',label),el('strong','',pct==null?'—':pct.toFixed(1)+'%'));d.append(head);if(pct!=null){const p=el('progress');p.max=100;p.value=pct;p.setAttribute('aria-label',label);d.append(p);if(pct>=85)d.classList.add('warning')}if(total)d.append(el('p','metric-detail',bytes(used)+' / '+bytes(total)));return d}
const liveReadings=new Map(),liveStreams=new Map();
function trend(history,value,label){
 const svg=document.createElementNS('http://www.w3.org/2000/svg','svg');svg.setAttribute('viewBox','0 0 240 54');svg.setAttribute('preserveAspectRatio','none');svg.setAttribute('class','trend');svg.setAttribute('role','img');
 const end=now(),start=end-600,buckets=new Map();
 for(const sample of history){const v=sample.status==='available'?value(sample):null;if(v==null||sample.checked<start||sample.checked>end)continue;const i=Math.min(29,Math.floor((sample.checked-start)/20));if(!buckets.has(i)||v>buckets.get(i).v)buckets.set(i,{v,sample})}
 // Keep CPU anchored at zero. Zoom RAM around its observed range; disk is capacity.
 const values=[...buckets.values()].map(x=>x.v);let low=0,high=100;
 if(values.length&&(label==='CPU'||label==='GPU'))high=Math.min(100,Math.max(10,Math.ceil(Math.max(...values)/10)*10));
 if(values.length&&label==='RAM'){low=Math.max(0,Math.floor(Math.min(...values)/5)*5);high=Math.min(100,Math.max(low+10,Math.ceil(Math.max(...values)/5)*5));low=Math.min(low,high-10)}
 svg.dataset.range=low+'–'+high+'%';svg.setAttribute('aria-label',label+' usage history: last 10 minutes, scale '+low+' to '+high+' percent');
 for(let i=0;i<30;i++){const entry=buckets.get(i),n=document.createElementNS(svg.namespaceURI,'rect');n.setAttribute('x',i*8);n.setAttribute('width',5);n.setAttribute('rx',1);const h=entry?Math.max(1,Math.min(1,Math.max(0,(entry.v-low)/(high-low)))*50):1;n.setAttribute('y',53-h);n.setAttribute('height',h);n.setAttribute('class',entry?'history-bar':'history-empty');const title=document.createElementNS(svg.namespaceURI,'title');title.textContent=entry?label+' '+entry.v.toFixed(1)+'% · '+new Date(entry.sample.checked*1000).toLocaleTimeString():'No sample';n.append(title);svg.append(n)}return svg;
}
function refreshVisible(){
 const wanted=new Set();
 if(!document.hidden&&view==='machines')for(const card of document.querySelectorAll('.machine-card')){
  const b=card.getBoundingClientRect(),r=data.machines.find(x=>x.id===card.dataset.id);
  if(b.bottom>0&&b.top<innerHeight&&r&&r.metrics?.status!=='unconfigured')wanted.add(r.id);
 }
 for(const [id,source] of liveStreams)if(!wanted.has(id)){source.close();liveStreams.delete(id)}
 for(const id of wanted){
  if(liveStreams.has(id))continue;
  const source=new EventSource('/api/metrics/stream?machine='+encodeURIComponent(id));liveStreams.set(id,source);
  const update=()=>{const r=data.machines.find(x=>x.id===id),box=document.querySelector('.machine-card[data-id="'+id+'"] .metrics-panel');if(r&&box)box.replaceWith(metricsPanel(r))};
  source.addEventListener('metrics',event=>{try{liveReadings.set(id,JSON.parse(event.data));update()}catch{source.close();liveStreams.delete(id)}});
  source.onerror=()=>{const old=liveReadings.get(id)||{};liveReadings.set(id,{...old,status:'unavailable',error:'Stream disconnected; reconnecting'});update()};
 }
}
let visibleTimer;addEventListener('scroll',()=>{clearTimeout(visibleTimer);visibleTimer=setTimeout(refreshVisible,100)},{passive:true});addEventListener('resize',refreshVisible);document.addEventListener('visibilitychange',()=>{refreshVisible();if(!document.hidden)refresh()});addEventListener('pagehide',()=>{for(const source of liveStreams.values())source.close();liveStreams.clear()});
function metricsPanel(r){
 const box=el('section','metrics-panel telemetry-strip'),m=liveReadings.get(r.id)||r.metrics||{},fresh=m.status==='available'&&(!m.expires||m.expires>now()),live=liveReadings.has(r.id)&&fresh;
 box.setAttribute('aria-label',r.host+' resource history');
 const disk=x=>{const ds=(x.disks||[]).filter(d=>d.total&&(!d.checked||x.checked-d.checked<=120));return ds.length?Math.max(...ds.map(d=>100*d.used/d.total)):null};
 const series=[['CPU',x=>x.cpu_percent],['RAM',x=>x.memory_total?100*x.memory_used/x.memory_total:null],['GPU',x=>x.gpu?.status==='available'&&Number.isFinite(x.gpu.percent)?x.gpu.percent:null],['DISK',disk]];
 const history=m.history||[],grid=el('div','telemetry-charts');
 for(const [label,value] of series){const cell=el('div','telemetry-cell '+label.toLowerCase()),head=el('div','telemetry-label');const current=fresh?value(m):null;head.append(el('span','',label),el('span','telemetry-value',current==null?'—':current.toFixed(1)+'%'));const chart=trend(history,value,label);const wrap=el('div','telemetry-plot');wrap.append(chart);
 if(label==='GPU'){
  const state=fresh?m.gpu?.status:null;
  cell.title=m.gpu?.note||'GPU monitoring requires an updated collector';
  if(state!=='available'){
   const absent=state==='not_applicable',overlay=el('div','telemetry-overlay',absent?'N/A':'Unavailable');
   overlay.setAttribute('role','status');overlay.setAttribute('aria-label',absent?'GPU: no accelerator available':'GPU: utilization unavailable');
   wrap.append(overlay);chart.setAttribute('aria-hidden','true');
  }
 }
 cell.append(head,wrap,el('div','telemetry-range',label==='GPU'&&(!fresh||m.gpu?.status!=='available')?(fresh&&m.gpu?.status==='not_applicable'?'No GPU':'No reading'):chart.dataset.range+' scale'));if(label==='DISK')cell.title='Most-used storage pool · '+(m.disks||[]).map(d=>d.name+': '+(100*d.used/d.total).toFixed(1)+'%').join(' · ');grid.append(cell)}
 const caption=el('div','telemetry-caption');caption.append(el('span','telemetry-state'+(live?' is-live':''),live?'Live · '+new Date(m.checked*1000).toLocaleTimeString([], {hour:'2-digit',minute:'2-digit',second:'2-digit'}):fresh?'Cached history':m.status==='waiting'?'Connecting stream':m.status==='unconfigured'?'Monitoring not configured':'Readings unavailable'),el('span','','10 min → now'));if(m.warnings?.length){caption.firstChild.textContent+=' · warning';caption.title=m.warnings.join('; ')}box.append(grid,caption);return box;
}
function backup(r){const b=r.config_backup||{},label=b.status==='scheduled'?'Scheduled':b.status==='manual'?'Manual copy':'Not recorded';const d=details(r.id+':backup','backup','Configuration backup · '+label);for(const l of b.locations||[])d.append(el('p','backup-location',l.host+' / '+l.path));if(!(b.locations||[]).length)d.append(el('p','','No configuration backup location recorded.'));if(b.notes)d.append(el('p','',b.notes));d.append(el('p','',r.health?.evidence||'No health evidence.'));const checked=el('p','','Last health check: ');if(r.health?.checked)checked.append(el('time','',new Date(r.health.checked*1000).toLocaleString()));else checked.append('Not recorded');d.append(checked);if(health(r)==='stale')d.append(el('p','','The last report expired. Current health is unverified.'));return d}
function cardHeader(r,machine){const head=el('header','card-head');if(machine)head.append(symbol());const identity=el('div','identity');identity.append(el('h3','',machine?r.host:r.name));if(machine)identity.append(el('p','',[r.model,r.os].filter(Boolean).join(' · ')));else identity.append(el('p','','Hosted on '+r.host));const hb=el('div','health-block'),s=health(r),labels=machine?{healthy:'Reachable',unhealthy:'Check failed',unknown:'Unknown',stale:'Stale'}:{healthy:'Healthy',unhealthy:'Unhealthy',unknown:'Unknown',stale:'Stale'};hb.append(el('span','badge '+s,labels[s]||s));const ago=el('p','check-age');ago.append(timeElement(r.health?.checked));hb.append(ago);head.append(identity,hb);return head}
function machineCard(r){const c=el('article','card machine-card');c.dataset.id=r.id;c.append(cardHeader(r,true),metricsPanel(r),accessPanel(r));const foot=el('div','card-foot'),hosted=data.services.filter(s=>s.machine_id===r.id),b=el('button','text-button',hosted.length+' services on this machine →');b.onclick=()=>{switchView('services');$('host').value=r.host;render()};foot.append(b);c.append(foot,backup(r));return c}
function serviceCard(r){const c=el('article','card service-card');c.dataset.id=r.id;c.append(cardHeader(r,false));const body=el('div','service-body');if(r.deployment)body.append(el('p','service-description',r.deployment));const links=el('div','service-links');for(const [label,value]of Object.entries(r.endpoints||{})){const u=safeUrl(value);if(!u)continue;if(['localhost','127.0.0.1','[::1]'].includes(u.hostname)){links.append(el('p','no-endpoint',label+' is local to '+r.host));continue}links.append(link('Open '+label+' ↗',u.href,'button secondary'))}if(!links.children.length)links.append(el('p','no-endpoint','No browser endpoint registered.'));body.append(links);const tags=el('div','capabilities');for(const t of r.capabilities||[])tags.append(el('span','capability',t));body.append(tags);c.append(body);const foot=el('div','card-foot'),b=el('button','text-button','View '+r.host+' →');b.onclick=()=>{switchView('machines');$('host').value=r.host;render()};foot.append(b);c.append(foot,backup(r));return c}
function render(){
 const machine=view==='machines';$('machines-tab').setAttribute('aria-pressed',String(machine));$('services-tab').setAttribute('aria-pressed',String(!machine));$('machine-count').textContent=data.machines.length;$('service-count').textContent=data.services.length;$('title').textContent=machine?'Machines':'Services';$('view-description').textContent=machine?'Your hardware, with every way in.':'What’s running, where it lives, and how it’s doing.';$('search').placeholder=machine?'Find a machine…':'Find a service…';
 const selected=$('host').value,hosts=[...new Set(data[view].map(r=>r.host))].sort();$('host').replaceChildren(new Option('All hosts',''),...hosts.map(h=>new Option(h,h)));if(hosts.includes(selected))$('host').value=selected;
 const controls=document.createDocumentFragment();for(const state of ['all',...statuses]){const label=state==='all'?'All':machine?({healthy:'Reachable',unhealthy:'Check failed',unknown:'Unknown',stale:'Stale'}[state]):state[0].toUpperCase()+state.slice(1);const n=state==='all'?data[view].length:data[view].filter(r=>health(r)===state).length;const b=el('button','stat',label);b.append(el('span','count',n));b.dataset.filter=state;b.setAttribute('aria-pressed',String(filter===state));b.onclick=()=>{filter=state;render()};controls.append(b)}$('health-filters').replaceChildren(controls);
 const q=$('search').value.toLowerCase(),host=$('host').value,rows=data[view].filter(r=>(filter==='all'||health(r)===filter)&&(!host||r.host===host)&&[r.name,r.host,r.id,...r.capabilities].join(' ').toLowerCase().includes(q));const rank={unhealthy:0,stale:1,unknown:2,healthy:3};rows.sort((a,b)=>rank[health(a)]-rank[health(b)]||a.name.localeCompare(b.name));$('summary').textContent=rows.length+' of '+data[view].length+' '+view;$('updated').textContent=data.fetched_at?'Updated '+new Date(data.fetched_at*1000).toLocaleTimeString([],{hour:'numeric',minute:'2-digit'}):'Connecting to the lab…';
 const cards=$('cards');cards.className=view;cards.setAttribute('aria-label',machine?'Machines':'Services');const root=document.createDocumentFragment();for(const r of rows)root.append(machine?machineCard(r):serviceCard(r));if(!rows.length)root.append(el('p','empty-results','No '+view+' match these filters.'));cards.replaceChildren(root);queueMicrotask(refreshVisible);
}
async function refresh(){if(busy||document.hidden)return;busy=true;$('refresh').disabled=true;try{const response=await fetch('/api/inventory',{signal:AbortSignal.timeout(12000),cache:'no-store'});if(!response.ok)throw Error('failed');data=await response.json();if(data.server_time)offset=data.server_time-Date.now()/1000;$('error').hidden=!data.error;$('error').textContent=data.error||'';if(!copying&&!$('cards').contains(document.activeElement))render();}catch{$('error').hidden=false;$('error').textContent='Unable to refresh. Displayed readings may be stale.';}finally{busy=false;$('refresh').disabled=false}}
$('machines-tab').onclick=()=>switchView('machines');$('services-tab').onclick=()=>switchView('services');$('search').oninput=render;$('host').onchange=render;$('refresh').onclick=refresh;document.querySelector('.brand').onclick=e=>{e.preventDefault();switchView('machines')};$('search').value=new URLSearchParams(location.search).get('q')||'';refresh();setInterval(refresh,30000);setInterval(()=>{if(!document.hidden&&!copying&&!$('cards').contains(document.activeElement))render()},15000);
