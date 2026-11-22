'use strict';
const $=id=>document.getElementById(id);
const views = ['machines','discrepancies','services','backups','map','leases'];
if (location.pathname === '/leases') history.replaceState(null, '', '/' + location.search + '#leases');
const initialView = location.hash.slice(1);
let data={machines:[],services:[],backups:[]},view=views.includes(initialView)?initialView:'machines',filter='all',busy=false,offset=0,copying=0;
let disposeGraph=()=>{};
const opened=new Set(),feedback=new Map(),statuses=['healthy','unhealthy','stale','unknown','stopped','running'];
const now=()=>Date.now()/1000+offset;
function el(tag,cls,text){const n=document.createElement(tag);if(cls)n.className=cls;if(text!==undefined)n.textContent=text;return n}
function age(t){if(!t)return 'No reading';const m=Math.max(0,Math.floor((now()-t)/60));if(m<1)return 'Just now';if(m<60)return m+'m ago';if(m<1440)return Math.floor(m/60)+'h ago';return Math.floor(m/1440)+'d ago'}
function bytes(n){return n==null?'Unknown':(n/1073741824).toFixed(1)+' GiB'}
function health(r){if(r.runtime?.expires){if(r.runtime.expires<now())return 'unknown';if(['stopped','running','unknown'].includes(r.runtime.state))return r.runtime.state}const h=r.health||{};return h.expires&&h.expires<now()?'stale':h.status||'unknown'}
function safeUrl(value,ssh=false){try{const u=new URL(value);return (ssh?['http:','https:','ssh:']:['http:','https:']).includes(u.protocol)&&!u.password?u:null}catch{return null}}
function link(label,url,cls='button primary'){const a=el('a',cls,label);a.href=url;a.target='_blank';a.rel='noopener noreferrer';return a}
function symbol(){const n=el('span','machine-symbol');n.setAttribute('aria-hidden','true');n.innerHTML='<svg viewBox="0 0 24 24"><rect x="4" y="3" width="16" height="7" rx="2"/><rect x="4" y="14" width="16" height="7" rx="2"/><path d="M8 6.5h.01M8 17.5h.01M13 6.5h4M13 17.5h4"/></svg>';return n}
function details(key,cls,label){const d=el('details',cls);d.open=opened.has(key);d.append(el('summary','',label));d.addEventListener('toggle',()=>{if(d.isConnected){if(d.open)opened.add(key);else opened.delete(key)}});return d}
function timeElement(t){const d=el('time','',age(t));if(t){d.dateTime=new Date(t*1000).toISOString();d.title=new Date(t*1000).toLocaleString()}return d}
function switchView(v,fromHistory=false){view=v;filter='all';$('search').value='';$('host').value='';if(!fromHistory&&location.hash!=='#'+v)history.pushState(null,'','#'+v);render()}
function setFeedback(key,node,message){feedback.set(key,message);node.textContent=message.startsWith('Could not copy')?'Copy failed':message;node.parentElement.title=message;$('toast').textContent=message}
function copyPassword(r,a,status){const b=el('button','button secondary password-copy');b.type='button';for(const label of ['Copy password','Password copied','Copying…','Copy failed']){const size=el('span','copy-size',label);size.setAttribute('aria-hidden','true');b.append(size)}status.className='copy-label';const previous=feedback.get(r.id+':'+a.id)||'Copy password';status.textContent=previous.startsWith('Could not copy')?'Copy failed':previous;b.title=previous;b.append(status);b.setAttribute('aria-label','Copy '+(r.display_name||r.host)+' '+a.label+' password');
 b.onclick=async()=>{b.disabled=true;copying++;const key=r.id+':'+a.id;setFeedback(key,status,'Copying…');try{
  if(!navigator.clipboard||!window.ClipboardItem)throw Error('unsupported');
  const value=fetch('api/copy-password',{method:'POST',headers:{'Content-Type':'application/json','X-Lab-Action':'copy-password'},body:JSON.stringify({machine:r.id,login:a.id}),cache:'no-store',signal:AbortSignal.timeout(15000)}).then(r=>{if(!r.ok)throw Error('failed');return r.text()}).then(text=>new Blob([text],{type:'text/plain'}));
  await navigator.clipboard.write([new ClipboardItem({'text/plain':value})]);setFeedback(key,status,'Password copied');
 }catch{setFeedback(key,status,'Could not copy. Check clipboard permission and retry.')}finally{b.disabled=false;copying--}};return b}
function accessPanel(r){const box=el('section','access-panel');box.setAttribute('aria-label',r.host+' login paths');const head=el('div','section-kicker','CONNECT');head.append(el('span','plain','Your ways into '+(r.display_name||r.host)));box.append(head);const grid=el('div','access-grid'),unavailable=[];
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
  if(ssh&&a.browser_terminal&&a.browser_terminal.startsWith('/terminal/')){const l=link('Open SSH terminal ↗',a.browser_terminal);l.setAttribute('aria-label','Open SSH terminal · '+(r.display_name||r.host));actions.append(l)}else{const label=ssh?'Open SSH app':a.id==='proxmox'?'Open Proxmox':a.id==='kvm'?'Open KVM':a.id==='console'?'Open terminal login':'Open Intel AMT';const l=link(label+' ↗',url.href);l.setAttribute('aria-label',label+' · '+r.host+(a.id==='amt-off'?' · OS off':''));actions.append(l)}row.append(actions);
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
function hasMetrics(r){return health(r)!=='stopped'&&!!r.metrics?.status&&r.metrics.status!=='unconfigured'}
function refreshVisible(){
 const wanted=new Set();
 if(!document.hidden&&view==='machines')for(const card of document.querySelectorAll('.machine-card')){
  const b=card.getBoundingClientRect(),r=data.machines.find(x=>x.id===card.dataset.id);
  if(b.bottom>0&&b.top<innerHeight&&r&&hasMetrics(r))wanted.add(r.id);
 }
 for(const [id,source] of liveStreams)if(!wanted.has(id)){source.close();liveStreams.delete(id)}
 for(const id of wanted){
  if(liveStreams.has(id))continue;
  const source=new EventSource('api/metrics/stream?machine='+encodeURIComponent(id));liveStreams.set(id,source);
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
function cardHeader(r,machine){const head=el('header','card-head');if(machine)head.append(symbol());const identity=el('div','identity');identity.append(el('h3','',machine?(r.display_name||r.host):r.name));if(machine)identity.append(el('p','',[r.model,r.os].filter(Boolean).join(' · ')));else identity.append(el('p','','Hosted on '+r.host));const hb=el('div','health-block'),s=health(r),labels=machine?{healthy:'Reachable',unhealthy:'Check failed',unknown:'Unknown',running:'Running',stale:'Stale'}:{healthy:'Healthy',unhealthy:'Unhealthy',unknown:'Unknown',running:'Running',stale:'Stale'};hb.append(el('span','badge '+s,s==='stopped'?'Stopped':labels[s]||s));const ago=el('p','check-age');ago.append(timeElement(['stopped','running'].includes(s)?r.runtime?.checked:r.health?.checked));hb.append(ago);head.append(identity,hb);return head}
function machineCard(r){const c=el('article','card machine-card');c.dataset.id=r.id;c.append(cardHeader(r,true));const check=hostInventory(r);if(check)c.append(check);if(hasMetrics(r))c.append(metricsPanel(r));if(r.access?.length)c.append(accessPanel(r));const foot=el('div','card-foot'),hosted=data.services.filter(s=>s.machine_id===r.id),b=el('button','text-button',hosted.length+' services on this machine →');b.onclick=()=>{switchView('services');$('host').value=r.host;render()};foot.append(b);c.append(foot,backup(r));return c}
function serviceCard(r){const c=el('article','card service-card');c.dataset.id=r.id;c.append(cardHeader(r,false));const body=el('div','service-body');if(r.deployment)body.append(el('p','service-description',r.deployment));const links=el('div','service-links');for(const [label,value]of Object.entries(r.endpoints||{})){const u=safeUrl(value);if(!u)continue;if(['localhost','127.0.0.1','[::1]'].includes(u.hostname)){links.append(el('p','no-endpoint',label+' is local to '+r.host));continue}links.append(link('Open '+label+' ↗',u.href,'button secondary'))}if(!links.children.length)links.append(el('p','no-endpoint','No browser endpoint registered.'));body.append(links);for(const a of r.access||[]){const credentials=el('div','credential-line'),user=el('div','username');user.append(el('span','username-label','Username'),el('div','username-value',a.username||(a.username_required===false?'Not required':'Not recorded')));credentials.append(user);if(a.password_available)credentials.append(copyPassword(r,a,el('span')));body.append(credentials)}const tags=el('div','capabilities');for(const t of r.capabilities||[])tags.append(el('span','capability',t));body.append(tags);c.append(body);const foot=el('div','card-foot'),b=el('button','text-button','View '+r.host+' →');b.onclick=()=>{switchView('machines');$('host').value=r.host;render()};foot.append(b);c.append(foot);if(r.kind!=='backup')c.append(backup(r));else c.append(el('p','service-body',r.health?.evidence||'No backup result recorded.'));return c}
function render(){
 disposeGraph();
 for (const v of views) {
   const button = $(v+'-tab'), selected = view === v;
   if (selected && !button.hidden && button.getAttribute('aria-pressed') !== 'true') {
     const nav = button.parentElement, bounds = nav.getBoundingClientRect(), tab = button.getBoundingClientRect();
     if (tab.right > bounds.right) nav.scrollLeft += tab.right - bounds.right;
     if (tab.left < bounds.left) nav.scrollLeft -= bounds.left - tab.left;
   }
   button.setAttribute('aria-pressed', String(selected && !button.hidden));
 }
 $('machine-count').textContent=data.machines.length;$('service-count').textContent=data.services.length;$('backup-count').textContent=data.backups.length;
 const leases = view === 'leases';
 const discrepancies = view === 'discrepancies';
 const enteringLeases = leases && $('lease-panel').hidden;
 $('inventory-panel').hidden = leases || discrepancies;
 $('lease-panel').hidden = !leases;
 $('discrepancies-panel').hidden = !discrepancies;
 $('updated').hidden = leases;
 const checked=data.catalog?.snapshot_at||data.catalog?.checked||data.fetched_at;
 const clock=checked?new Date(checked*1000).toLocaleTimeString([],{hour:'numeric',minute:'2-digit'}):'';
 const source=data.catalog?.snapshot_at?'Preview catalog copied ':'Catalog last read ';
 const label=clock?source+clock:'Connecting to the lab…';
 $('updated').textContent=data.catalog&&data.catalog.feed!=='connected'?'Catalog feed unavailable · '+label:label;
 if (leases) {
   if (enteringLeases) {
     refreshVisible();
     LeaseHistory.open();
   }
   return;
 }
 if (discrepancies) {
   refreshVisible();
   return;
 }
 const machine=view==='machines',isBackup=view==='backups',map=view==='map',items=map?[...data.machines,...data.services]:data[view];
 $('title').textContent=map?'Resources':machine?'Machines':isBackup?'Backups':'Services';
 $('view-description').textContent=map?'Physical machines, their VMs and containers, and the services they run.':machine?'Physical hosts and virtual environments. Stopped states are labeled with their observation time.':isBackup?'Scheduled jobs handled by the central backup service.':'Registered applications and infrastructure. Health reports do not imply every service is running.';
 $('search').placeholder=map?'Find a machine or service…':machine?'Find a machine…':isBackup?'Find a backup job…':'Find a service…';
 const selected=$('host').value,hosts=[...new Set(items.map(r=>r.host))].sort();$('host').replaceChildren(new Option('All hosts',''),...hosts.map(h=>new Option(h,h)));if(hosts.includes(selected))$('host').value=selected;
 const controls=document.createDocumentFragment();for(const state of ['all',...statuses]){const label=state==='all'?'All':machine?({healthy:'Reachable',unhealthy:'Check failed',unknown:'Unknown',running:'Running',stale:'Stale',stopped:'Stopped',running:'Running'}[state]):state[0].toUpperCase()+state.slice(1);const n=state==='all'?items.length:items.filter(r=>health(r)===state).length;const b=el('button','stat',label);b.append(el('span','count',n));b.dataset.filter=state;b.setAttribute('aria-pressed',String(filter===state));b.onclick=()=>{filter=state;render()};controls.append(b)}$('health-filters').replaceChildren(controls);
 const q=$('search').value.toLowerCase(),host=$('host').value,rows=items.filter(r=>(filter==='all'||health(r)===filter)&&(!host||r.host===host)&&[r.name,r.host,r.id,...r.capabilities].join(' ').toLowerCase().includes(q));const rank={unhealthy:0,stale:1,unknown:2,healthy:3,running:3,stopped:4};rows.sort((a,b)=>rank[health(a)]-rank[health(b)]||a.name.localeCompare(b.name));$('summary').textContent=rows.length+' of '+items.length+' '+(map?'resources':view);
 const cards=$('cards');cards.className=map?'':machine?'machines':'services';cards.setAttribute('aria-label',map?'Resources':machine?'Machines':isBackup?'Backups':'Services');const root=document.createDocumentFragment();if(!map)for(const r of rows)root.append(machine?machineCard(r):serviceCard(r));if(!rows.length)root.append(el('p','empty-results','No '+(map?'resources':view)+' match these filters.'));cards.replaceChildren(root);if(map&&rows.length)disposeGraph=ResourceGraph.mount(cards,items.map(r=>({...r,name:r.display_name||r.name,kind:r.kind==='machine'?r.machine_type:'service',status:health(r)})),{matching:new Set(rows.map(r=>r.id))});queueMicrotask(refreshVisible);
}
async function refresh(){if(busy||document.hidden)return;busy=true;$('refresh').disabled=true;try{const response=await fetch('api/inventory',{signal:AbortSignal.timeout(12000),cache:'no-store'});if(!response.ok)throw Error('failed');data=await response.json();$('leases-tab').hidden=!data.lease_review_enabled;if(view==='leases'&&!data.lease_review_enabled){view='machines';history.replaceState(null,'','#machines')}if(data.server_time)offset=data.server_time-Date.now()/1000;$('error').hidden=!data.error;$('error').textContent=data.error||'';showDiscrepancies();if(!copying&&!$('cards').contains(document.activeElement))render();}catch{$('error').hidden=false;$('error').textContent='Unable to refresh. Displayed readings may be stale.';}finally{busy=false;$('refresh').disabled=false}}
async function refreshCatalog(){
 const button=$('refresh');
 button.disabled=true;
 try{
  const r=await fetch('api/refresh',{method:'POST',headers:{'Content-Type':'application/json','X-Lab-Action':'refresh-catalog'},body:'{}',signal:AbortSignal.timeout(8000)});
  if(!r.ok)throw Error('HTTP '+r.status);
  await refresh();
 }catch(e){
  $('error').hidden=false;
  $('error').textContent='Catalog refresh failed: '+e.message;
 }finally{button.disabled=false;}
}
for (const name of views) $(name+'-tab').onclick = () => switchView(name);
addEventListener('popstate', () => {
  const name = location.hash.slice(1) || 'machines';
  if (views.includes(name)) switchView(name,true);
});
$('search').oninput=render;$('host').onchange=render;$('refresh').onclick=()=>view==='leases'?LeaseHistory.load():refreshCatalog();document.querySelector('.brand').onclick=e=>{e.preventDefault();switchView('machines')};$('search').value=new URLSearchParams(location.search).get('q')||'';

let reconcileRequested=0;
const findingGroups=[
 ['guest-mismatch','Confirmed guest mismatches'],
 ['record-review','Deployment records to review'],
 ['machine-health','Machine health'],
 ['service-health','Service health'],
 ['backup-health','Backup jobs'],
 ['network','IP reservation audit'],
 ['lease','Environment leases'],
 ['warning','Warnings'],
 ['coverage','Checks we could not verify']
];
function findingItem(f){
 return el('li','',f.target+(f.guest_id?' / '+f.guest_id:'')+' — '+f.message+(f.name?' ('+f.name+')':''));
}
function hostInventory(machine){
 const report=data.inventory_accuracy;
 const host=report?.hosts.find(h=>h.machine_id===machine.id);
 if(!host)return null;
 const verified=host.status==='verified'&&host.expires>now()&&!data.error;
 const label=verified?'Guest inventory verified':host.status==='unconfigured'?'Guest inventory collector not configured':'Guest inventory unverified';
 const panel=el('div','host-inventory');
 panel.append(el('strong','',label));
 if(verified){panel.append(document.createTextNode(' · '),timeElement(host.checked))}
 const hardware=host.hardware;
 if(hardware && hardware.expires>now() && !data.error){
  const facts=el('div','',hardware.architecture+' '+hardware.os+' · '+hardware.logical_cpus+
   ' logical CPUs · '+bytes(hardware.memory_bytes)+' RAM · Hardware checked ');
  facts.append(timeElement(hardware.checked));
  panel.append(facts);
 }else if(host.status!=='unconfigured'){
  panel.append(el('div','',hardware && hardware.expires<=now()?'Hardware report expired':'Hardware not verified'));
 }
 const issues=report.findings.filter(f=>
  (f.machine_id===machine.id||f.target===machine.id)&&
  (f.category!=='guest-mismatch'||verified));
 if(issues.length){
  const list=details('reconciliation:'+machine.id,'',issues.length+' inventory '+(issues.length===1?'item':'items')+' to review');
  const rows=el('ul');for(const f of issues)rows.append(findingItem(f));
  list.append(rows);panel.append(list);
 }
 return panel;
}
function healthFindings() {
 const findings=[];
 for(const [collection,category] of [['machines','machine-health'],['services','service-health'],['backups','backup-health']]){
  for(const record of data[collection]){
   if(record.lifecycle!=='active')continue;
   const state=record.health||{};
   if(category==='machine-health'&&health(record)==='stopped')continue;
   const target=record.display_name||record.name;
   if(!data.error&&state.status==='unhealthy'&&state.expires>now()){
    findings.push({category,target,message:state.evidence||'Health check failed.'});
   }else if(state.status==='stale'||(state.expires&&state.expires<=now())){
    findings.push({category:'coverage',target,message:'The last health check expired.'});
   }else if(state.status==='unknown'&&(state.checked||category==='backup-health')){
    findings.push({category:'coverage',target,message:category==='backup-health'?'No verified backup result.':'Health check inconclusive.'});
   }
  }
 }
 return findings;
}

function addressFindings(a) {
 const target='IP reservation audit';
 if(!a)return [{category:'coverage',target,message:'No audit result has been published.'}];
 const date=new Date(a.at).toLocaleString();
 const findings=[];
 if(a.status==='error'||a.status==='failed'){
  findings.push({category:'network',target,message:'The audit failed on '+date+'.'});
 }else if(a.unreserved_static>0){
  findings.push({category:'network',target,count:a.unreserved_static,
   message:a.unreserved_static+' unreserved static '+(a.unreserved_static===1?'address':'addresses')+' in the audit from '+date+'.'});
 }
 if(a.warnings>0){
  findings.push({category:'warning',target,count:a.warnings,
   message:a.warnings+' '+(a.warnings===1?'warning':'warnings')+' in the audit from '+date+'.'});
 }
 return findings;
}

function leaseFindings(report){
 if(!report)return [];
 if(report.status!=='verified')return [{category:'coverage',target:'Leases',message:
  report.status==='checking'?'Checking current leases.':'Could not check current leases.'}];
 LeaseHistory.showCounts(report);
 const messages={'export-failed':'Workspace export failed. See Leases for details.','overdue':'The lease is overdue.'};
 return report.issues.map(issue=>({category:'lease',target:issue.task,message:messages[issue.code]}));
}

function showDiscrepancies(){
 const a=data.inventory_accuracy;
 const visibleFindings=[...healthFindings(),...leaseFindings(data.lease_issues)];
 if(data.fetched_at)visibleFindings.push(...addressFindings(data.address_audit));
 if(data.error&&data.fetched_at)visibleFindings.push({category:'coverage',target:'Registry',message:data.error});
 if(data.catalog&&data.catalog.feed!=='connected'){
  visibleFindings.push({category:'coverage',target:'Registry catalog',message:'Live catalog updates unavailable. Cached records were last read '+(data.catalog.checked?age(data.catalog.checked).toLowerCase():'at startup')+'.'});
 }
 const count=$('discrepancy-count');
 const summary=$('accuracy-summary');
 const scope=$('accuracy-scope');
 const findings=$('accuracy-findings');
 const detailPanel=$('accuracy-details');
 const catalogUnverified=data.catalog&&(data.catalog.feed!=='connected'&&
  (!data.catalog.checked||now()-data.catalog.checked>90));
 const stale=Boolean(!a||data.error||now()-a.checked>90);
 const expiredHosts=new Set((a?.hosts||[]).filter(h=>h.status==='verified'&&h.expires<=now()).map(h=>h.machine_id));
 const c=a?.coverage;
 const coverage=c?(c.verified-expiredHosts.size)+' of '+c.expected+' configured hosts verified':'Guest inventory unavailable';
 if(a){
  const expiredDeployments=new Set((a.deployments||[]).filter(d=>d.state==='missing'&&
   (data.error||d.expires<=now())).map(d=>d.record_id));
  visibleFindings.push(...a.findings.filter(f=>
   (f.category!=='guest-mismatch'||(!stale&&!catalogUnverified&&!expiredHosts.has(f.machine_id||f.target)))&&
   (f.code!=='deployment-missing'||!expiredDeployments.has(f.target))));
  for(const target of expiredDeployments){
   visibleFindings.push({category:'coverage',target,message:'The deployment state could not be verified.'});
  }
 }
 const tally=items=>items.reduce((sum,item)=>sum+(item.count??1),0);
 const unverified=tally(visibleFindings.filter(f=>f.category==='coverage'));
 const warnings=tally(visibleFindings.filter(f=>f.category==='warning'));
 const discrepancies=tally(visibleFindings)-unverified-warnings;
 const badge=data.fetched_at?String(discrepancies):'—';
 if(count.textContent!==badge)count.textContent=badge;
 count.classList.toggle('has-errors',discrepancies>0);
 const result=!data.fetched_at?'Waiting for lab reports…':discrepancies?
  discrepancies+' '+(discrepancies===1?'problem':'problems'):'No confirmed problems';
 const status=[result];
 if(warnings)status.push(warnings+' '+(warnings===1?'warning':'warnings'));
 if(unverified)status.push(unverified+' unverified '+(unverified===1?'check':'checks'));
 const summaryText=status.join(' · ');
 if(summary.textContent!==summaryText)summary.textContent=summaryText;
 const audit=data.address_audit;
 scope.textContent=coverage+(stale&&c?' · Guest inventory needs a refresh':'')+
  (c?.unconfigured?' · '+c.unconfigured+' hosts have no configured guest collector':'')+
  (audit?' · Last IP audit '+new Date(audit.at).toLocaleDateString():'');
 detailPanel.hidden=visibleFindings.length===0;
 const next=JSON.stringify(visibleFindings);
 if(findings.dataset.items!==next){
  const groups=document.createDocumentFragment();
  for(const [category,title] of findingGroups){
   const items=visibleFindings.filter(f=>f.category===category);
   if(!items.length)continue;
   const label=title+' ('+tally(items)+')';
   const group=category==='coverage'?
    details('discrepancies:coverage','accuracy-group',label):el('section','accuracy-group');
   if(category!=='coverage')group.append(el('h3','',label));
   const list=el('ul');for(const f of items)list.append(findingItem(f));
   group.append(list);groups.append(group);
  }
  findings.replaceChildren(groups);
  findings.dataset.items=next;
 }
 finishComparison(data);
}
function finishComparison(report){
 const receipt=report.comparison;
 if(!reconcileRequested||receipt?.id!==reconcileRequested||receipt.state==='pending')return;
 reconcileRequested=0;
 const accuracy=report.inventory_accuracy;
 if(receipt.state==='failed'||!accuracy){
  $('reconcile-status').textContent='Could not complete reconciliation. Check registry connectivity and retry.';
 }else{
  const checks=accuracy.hosts.filter(host=>host.status==='verified').map(host=>host.checked);
  const oldest=checks.length?' · Oldest verified report: '+age(Math.min(...checks)):'';
  const {verified,expected}=accuracy.coverage;
  $('reconcile-status').textContent='Comparison finished · '+verified+' of '+expected+' configured hosts verified'+oldest;
 }
 $('reconcile').disabled=false;
}

$('reconcile').onclick=async()=>{
 const button=$('reconcile');button.disabled=true;$('reconcile-status').textContent='Checking registered deployments against current host inventories…';
 try{const r=await fetch('api/reconcile',{method:'POST',headers:{'Content-Type':'application/json','X-Lab-Action':'reconcile'},body:'{}',signal:AbortSignal.timeout(12000)});if(!r.ok)throw Error('HTTP '+r.status);
  reconcileRequested=(await r.json()).id;
  if(!reconcileRequested)throw Error('Server returned no comparison ID');
  finishComparison(data);
  const deadline=Date.now()+90000;
  while(reconcileRequested && Date.now()<deadline){
   await new Promise(resolve=>setTimeout(resolve,4000));
   if(!reconcileRequested)break;
   // A manual comparison can finish while this browser's event stream is disconnected.
   const result=await fetch('api/reconciliation',{cache:'no-store',signal:AbortSignal.timeout(8000)});
   if(!result.ok)throw Error('HTTP '+result.status+' reading comparison');
   const report=await result.json();
   if(report.comparison?.id===reconcileRequested&&report.comparison.state==='complete'){
    data.inventory_accuracy=report.inventory_accuracy;
    finishComparison(report);
    showDiscrepancies();
   }else finishComparison(report);
  }
  if(reconcileRequested)throw Error('Timed out waiting for a completed reconciliation');
 }catch(e){$('reconcile-status').textContent='Could not complete reconciliation: '+e.message;reconcileRequested=0}finally{button.disabled=false}
};

var inventoryStream, inventoryRetry, inventoryRetryDelay;
function connectInventory(){
 clearTimeout(inventoryRetry);
 if(inventoryStream)inventoryStream.close();
 if(document.hidden)return;
 inventoryStream=new EventSource('api/inventory/stream');
 inventoryStream.addEventListener('inventory',event=>{
  try{data=JSON.parse(event.data);inventoryRetryDelay=1000;if(data.server_time)offset=data.server_time-Date.now()/1000;
   $('error').hidden=!data.error;$('error').textContent=data.error||'';
   showDiscrepancies();
   if(!copying&&!$('cards').contains(document.activeElement))render();
  }catch{$('error').hidden=false;$('error').textContent='Invalid inventory event; waiting for a valid update.'}
 });
 inventoryStream.onerror=()=>{
  $('error').hidden=false;$('error').textContent='Inventory stream disconnected; reconnecting. Cached observations may be stale.';
  // EventSource retries transport errors, but closes permanently on some HTTP errors.
  if(inventoryStream.readyState===EventSource.CLOSED){
   clearTimeout(inventoryRetry);
   inventoryRetry=setTimeout(connectInventory,inventoryRetryDelay||1000);
   inventoryRetryDelay=Math.min(30000,(inventoryRetryDelay||1000)*2);
  }
 };
}
addEventListener('pagehide',()=>{clearTimeout(inventoryRetry);inventoryStream?.close()});
document.addEventListener('visibilitychange',connectInventory);
addEventListener('online',()=>{if(inventoryStream?.readyState!==EventSource.OPEN)connectInventory()});
addEventListener('pageshow',event=>{if(event.persisted){connectInventory();refreshVisible()}});
refresh();
connectInventory();
setInterval(()=>{
 if(document.hidden)return;
 showDiscrepancies();
 if(!copying&&!$('cards').contains(document.activeElement))render();
},15000);
