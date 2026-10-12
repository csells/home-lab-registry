"""Render actual dashboard GPU charts in an isolated runner with labeled fixtures."""
import functools,json,pathlib,threading,tempfile
from http.server import ThreadingHTTPServer,SimpleHTTPRequestHandler
from playwright.sync_api import sync_playwright
ROOT=pathlib.Path(__file__).resolve().parents[1]
class Quiet(SimpleHTTPRequestHandler):
 def log_message(self,*args):pass
server=ThreadingHTTPServer(('127.0.0.1',0),functools.partial(Quiet,directory=str(ROOT/'dashboard')))
threading.Thread(target=server.serve_forever,daemon=True).start()
try:
 with sync_playwright() as p:
  browser=p.chromium.launch()
  try:
   for width in [393,1440]:
    page=browser.new_page(viewport={'width':width,'height':900})
    page.route('**/api/inventory',lambda route:route.fulfill(json={'machines':[],'services':[],'fetched_at':0}))
    page.goto(f'http://127.0.0.1:{server.server_port}/')
    result=page.evaluate('''() => {
      const root=document.createElement('div');root.style.width='100%';document.body.append(root);
      const t=now(), history=Array.from({length:30},(_,i)=>({checked:t-590+i*20,status:'available',gpu:{status:'available',percent:i%2?80:5}}));
      const cases=[['available',0],['available',25],['not_applicable',null],['unavailable',null],[null,null]];
      const results=cases.map(([state,percent],i)=>{
       const m={status:'available',checked:t,expires:t+30,cpu_percent:10,memory_used:5,memory_total:10,disks:[{name:'root',used:1,total:10}],history,gpu:state?{status:state,percent}:undefined};
       const panel=metricsPanel({id:'fixture-'+i,host:'Synthetic GPU test',metrics:m});root.append(panel);
       const gpu=panel.querySelector('.gpu'),bars=[...gpu.querySelectorAll('.history-bar')];
       return {state,value:gpu.querySelector('.telemetry-value').textContent,overlay:gpu.querySelector('.telemetry-overlay')?.textContent||null,cells:panel.querySelectorAll('.telemetry-cell').length,spread:Math.max(...bars.map(b=>+b.getAttribute('height')))-Math.min(...bars.map(b=>+b.getAttribute('height'))),fits:panel.scrollWidth<=panel.clientWidth};
      });return results;
    }''')
    assert all(x['cells']==4 and x['fits'] for x in result),result
    assert result[0]['value']=='0.0%' and result[0]['overlay'] is None,result
    assert result[1]['value']=='25.0%' and result[1]['spread']>20,result
    assert result[2]['overlay']=='N/A',result
    assert result[3]['overlay']==result[4]['overlay']=='Unavailable',result
    path=ROOT/f'gpu-review-{width}.png';page.screenshot(path=str(path),full_page=True)
    print(f'PASS {width}px: four charts, changing GPU bars, zero, N/A, unavailable and older collector',flush=True)
    page.close()
  finally:browser.close()
finally:server.shutdown();server.server_close()
