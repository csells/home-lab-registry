'use strict';
let records=[], loadedAt=null, filter='all', busy=false, failed=false, serverOffset=0;
const $=id=>document.getElementById(id), statuses=['healthy','unhealthy','stale','unknown'];
function node(tag,cls,text){const e=document.createElement(tag);if(cls)e.className=cls;if(text!==undefined)e.textContent=text;return e;}
function status(r){return r.health.expires && r.health.expires < Date.now()/1000+serverOffset ? 'stale' : statuses.includes(r.health.status)?r.health.status:'unknown';}
function age(t){if(!t)return 'Never checked';const s=Math.max(0,Math.floor(Date.now()/1000+serverOffset-t));if(s<60)return `${s}s ago`;if(s<3600)return `${Math.floor(s/60)}m ago`;if(s<86400)return `${Math.floor(s/3600)}h ago`;return `${Math.floor(s/86400)}d ago`;}
function render(){
 const opened=new Set([...document.querySelectorAll('details[open]')].map(d=>d.dataset.id));
 $('count-all').textContent=loadedAt?records.length:'—';for(const s of statuses)$('count-'+s).textContent=loadedAt?records.filter(r=>status(r)===s).length:'—';
 document.querySelectorAll('.stat').forEach(b=>{b.classList.toggle('selected',b.dataset.filter===filter);b.setAttribute('aria-pressed',String(b.dataset.filter===filter));});
 const q=$('search').value.toLowerCase(),host=$('host').value;
 const list=records.filter(r=>(filter==='all'||status(r)===filter)&&(!host||r.host===host)&&[r.name,r.id,r.host,...r.capabilities].join(' ').toLowerCase().includes(q));
 const rank={unhealthy:0,stale:1,unknown:2,healthy:3};list.sort((a,b)=>rank[status(a)]-rank[status(b)]||a.name.localeCompare(b.name));
 $('shown').textContent=loadedAt?`${list.length} / ${records.length}`:'';
 const content=document.createDocumentFragment();
 for(const r of list){
  const s=status(r),row=node('article','row'),identity=node('div');identity.append(node('div','name',r.name),node('div','tags',r.capabilities.join(' · ')));row.append(identity,node('div','hostname',r.host));
  const sc=node('div','status-cell'),badge=node('span','badge '+s);badge.append(node('i'),document.createTextNode(s));sc.append(badge);row.append(sc);
  const time=node('div','time-cell');time.append(node('div','age',age(r.health.checked)));if(r.health.checked)time.append(node('time','timestamp',new Date(r.health.checked*1000).toLocaleString()));row.append(time);
  const backup=r.config_backup||{status:'unknown',locations:[]},bc=node('div','backup-cell');
  const labels={unknown:'Not recorded',manual:'Manual copy',scheduled:'Scheduled'};
  bc.append(node('strong','', 'Config backup · '+(labels[backup.status]||'Not recorded')));
  for(const loc of backup.locations||[])bc.append(node('div','backup-location',loc.host+': '+loc.path));
  if(!(backup.locations||[]).length)bc.append(node('div','','No configuration backup location recorded.'));
  if(backup.notes)bc.append(node('div','',backup.notes));
  row.append(bc);
  const details=node('details','details');details.dataset.id=r.id;details.open=opened.has(r.id);details.append(node('summary','', 'Check details & endpoints'));
  const body=node('div','detail-body');body.append(node('p','',r.health.evidence||'No health observation has been published.'));
  if(s==='stale')body.append(node('p','','The last report has expired. Current health is unverified.'));
  const links=node('div','endpoints');for(const [label,url]of Object.entries(r.endpoints)){try{const u=new URL(url);if(!['http:','https:'].includes(u.protocol))continue;if(['127.0.0.1','localhost','[::1]'].includes(u.hostname)){links.append(node('span','',`${label}: host-local on ${r.host}`));continue;}const a=node('a','',label+' ↗');a.href=url;a.target='_blank';a.rel='noopener noreferrer';links.append(a);}catch{}}
  if(!links.children.length)links.append(node('span','','No browser endpoint registered.'));body.append(links,node('p','','Private LAN and tailnet links require access to that network.'));details.append(body);row.append(details);content.append(row);
 }
 if(!list.length)content.append(node('p','empty',loadedAt?'No services match these filters.':failed?'Registry unavailable. Use Refresh now to retry.':'Loading live service records…'));
 $('services').replaceChildren(content);
 if(loadedAt)$('synced').textContent=(failed?'Last successful refresh ':'Updated ')+new Date(loadedAt).toLocaleTimeString();
}
async function refresh(){if(busy)return;busy=true;$('refresh').disabled=true;try{const response=await fetch('/api/services',{cache:'no-store',signal:AbortSignal.timeout(18000)});if(!response.ok)throw new Error('Registry unavailable');const data=await response.json();records=data.services;loadedAt=Date.now();serverOffset=data.fetched_at-Date.now()/1000;failed=false;$('error').hidden=true;const selected=$('host').value;$('host').replaceChildren(new Option('All hosts',''),...[...new Set(records.map(r=>r.host))].sort().map(h=>new Option(h[0].toUpperCase()+h.slice(1),h)));if([...$('host').options].some(o=>o.value===selected))$('host').value=selected;}catch(e){failed=true;$('error').hidden=false;$('error').textContent='Registry unavailable — '+(loadedAt?'showing the last successful snapshot. Reports continue to expire.':'no live readings available.')+' Retrying every 30 seconds.';}finally{busy=false;$('refresh').disabled=false;render();}}
$('refresh').addEventListener('click',refresh);$('search').addEventListener('input',render);$('host').addEventListener('change',render);document.querySelectorAll('.stat').forEach(b=>b.addEventListener('click',()=>{filter=b.dataset.filter;render();}));refresh();setInterval(refresh,30000);setInterval(()=>{if(loadedAt)render();},15000);
