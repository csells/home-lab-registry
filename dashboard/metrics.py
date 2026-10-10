"""Read-only, command-restricted SSH machine measurements. No agent execution."""
import json,subprocess,time,concurrent.futures,pathlib,re

def parse_macos(text,checked):
    idle=float(re.findall(r'CPU usage:.*?([0-9.]+)% idle',text)[-1])
    total=int(re.search(r'hw.memsize: (\d+)',text)[1])
    page=int(re.search(r'page size of (\d+) bytes',text)[1])
    counts=[int(re.search(re.escape(name)+r':\s+(\d+)',text)[1]) for name in ['Pages active','Pages wired down','Pages occupied by compressor']]
    used=sum(counts)*page
    boot=int(re.search(r'kern.boottime:.*?sec = (\d+)',text)[1])
    disk=next(line.split() for line in text.splitlines() if line.rstrip().endswith('/System/Volumes/Data'))
    return {'status':'available','checked':checked,'expires':checked+1200,'source':'macOS top, vm_stat and df via restricted SSH','cpu_percent':round(100-idle,1),'memory_used':used,'memory_total':total,'memory_note':'Active + wired + compressed physical pages; excludes inactive cache.','disks':[{'name':'Data (APFS)','used':int(disk[2])*1024,'total':int(disk[1])*1024}],'uptime_seconds':max(0,checked-boot)}

def collect(target,private):
    checked=time.time()
    try:
        cmd=['ssh','-i',str(private/'metrics_key'),'-o','BatchMode=yes','-o','IdentitiesOnly=yes','-o','StrictHostKeyChecking=yes','-o','UserKnownHostsFile='+str(private/'known_hosts'),'-o','ConnectTimeout=5',target['user']+'@'+target['host']]
        result=subprocess.run(cmd,capture_output=True,text=True,timeout=15,check=True)
        if target['kind']=='proxmox':
            lines=result.stdout.splitlines();node=json.loads(lines[0]);stores=json.loads(lines[1]);sample=json.loads(lines[2])[0];memory=node['memory'];disks=[]
            for x in stores:
                if x.get('total') and x.get('active'):disks.append({'name':x['storage'],'used':x['used'],'total':x['total']})
            return {'status':'available','checked':checked,'expires':checked+1200,'source':'Proxmox node and storage API','cpu_percent':round(sample['cpu']*100,1),'memory_used':memory['used'],'memory_total':memory['total'],'disks':disks,'uptime_seconds':node['uptime']}
        if target['kind']=='macos':return parse_macos(result.stdout,checked)
        raise ValueError('Unsupported collector')
    except Exception as e:
        return {'status':'unavailable','checked':checked,'expires':checked+1200,'error':'Collection failed: '+type(e).__name__}

def collect_all(private):
    targets=json.loads((private/'targets.json').read_text())
    with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
        return dict(zip(targets,pool.map(lambda t:collect(t,private),targets.values())))

# Shared across viewers: one bounded history and one in-flight collection per host.
import threading
from collections import deque
class LiveMetrics:
    def __init__(self, private):
        self.private=private
        self.targets=json.loads((private/'targets.json').read_text())
        self.locks={key:threading.Lock() for key in self.targets}
        self.latest={};self.history={key:deque(maxlen=120) for key in self.targets}
    def read(self, key):
        if key not in self.targets:raise KeyError(key)
        with self.locks[key]:
            previous=self.latest.get(key)
            if previous is None or time.time()-previous['checked']>=15:
                reading=collect(self.targets[key],self.private)
                reading['checked']=time.time();reading['expires']=reading['checked']+45
                self.latest[key]=reading
                self.history[key].append(dict(reading))
            return {**self.latest[key],'history':list(self.history[key]),'interval_seconds':15}
