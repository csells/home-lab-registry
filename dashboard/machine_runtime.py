"""Small deterministic hypervisor state reads; no address audit or inference timer."""
import copy,json,pathlib,subprocess,time,logging
from network_audit import ssh_command,guest_identity

def read_runtime(target,private):
    script="import json,subprocess,platform\nif platform.system()=='Darwin':\n print(json.dumps({'tart':json.loads(subprocess.check_output(['/opt/homebrew/bin/tart','list','--format','json']))}))\nelse:\n print(json.dumps({'proxmox':json.loads(subprocess.check_output(['pvesh','get','/cluster/resources','--type','vm','--output-format','json']))}))"
    try:
        p=subprocess.run(ssh_command(target,private)+['sudo -n python3 -' if target.get('sudo') else 'python3 -'],input=script.encode(),capture_output=True,timeout=25)
        if p.returncode:raise RuntimeError('Runtime SSH failed')
        value=json.loads(p.stdout)
        if 'proxmox' in value: guests={str(v['vmid']):v['status'] for v in value['proxmox']}
        else: guests={v['Name']:('running' if v['Running'] else 'stopped') for v in value['tart'] if v.get('Source')=='local'}
        return {'guests':guests,'checked':time.time()}
    except Exception:
        logging.exception('Runtime read failed for %s',target['id'])
        return {'error':'Runtime source inaccessible','checked':time.time()}

def overlay(records,states,now=None):
    now=time.time() if now is None else now
    result=copy.deepcopy(records)
    for record in result:
        s=record['spec'];r=s.get('resources',{})
        if r.get('kind') not in ('machine','virtual-machine','container') and 'machine' not in s.get('capabilities',[]):continue
        parent,gid=guest_identity(s)
        if not parent or not gid:continue
        sample=states.get(parent,{})
        fresh=sample.get('checked') and now-sample['checked']<900
        state=sample.get('guests',{}).get(gid) if fresh and not sample.get('error') else None
        r['runtime']={'state':state if state in ('running','stopped') else 'unknown','checked':sample.get('checked'),'expires':sample.get('checked',0)+900,'evidence': 'Authenticated hypervisor inventory' if state else 'Runtime source unavailable, expired or guest missing'}
    return result

class RuntimeMonitor:
    def __init__(self,private):
        import threading
        self.private=pathlib.Path(private);self.lock=threading.Lock();self.states={};self.started=0;self.running=False
    def refresh(self,records):
        import threading
        with self.lock:
            if self.running or time.time()-self.started<600:return
            self.running=True;self.started=time.time()
        threading.Thread(target=self._read,args=(records,),daemon=True).start()
    def _read(self,records):
        import concurrent.futures
        from urllib.parse import urlsplit
        try:
            cfg=json.loads((self.private/'network-audit.json').read_text());by_id={r['id']:r['spec'] for r in records}
            for target in cfg['targets']:
                host=urlsplit(by_id.get(target['id'],{}).get('endpoints',{}).get('ssh','')).hostname
                if host:target['destination']=target['destination'].split('@')[0]+'@'+host
            with concurrent.futures.ThreadPoolExecutor(max_workers=3) as pool:
                found=list(pool.map(lambda t:(t['id'],read_runtime(t,self.private)),cfg['targets']))
            with self.lock:self.states=dict(found)
        except Exception:logging.exception('Runtime monitoring failed')
        finally:
            with self.lock:self.running=False
    def apply(self,records):
        with self.lock:return overlay(records,self.states)
