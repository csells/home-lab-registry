"""Guest power state read from each machine's mutually authenticated metrics agent."""
import concurrent.futures,copy,json,logging,math,threading,time,urllib.error
from urllib.parse import urlsplit,urlunsplit
from http.client import HTTPException
from metrics import connect
from network_audit import guest_identity

MAX_REPORT=4<<20
MAX_AGE=900  # Never trust a guest state older than this, whatever the agent claims.

def guests_url(stream_url):
    u=urlsplit(stream_url)
    return urlunsplit((u.scheme,u.netloc,'/guests','',''))

def guest_endpoints(endpoints,private=None):
    """Apply an optional private machine -> {url,server_name} mapping for guest inventory.

    Lets an operator read guest state from a different agent (for example a staging
    candidate) without writing the registry; unlisted machines keep their stream agent."""
    result=dict(endpoints)
    for key,cfg in (private or {}).items():
        url=cfg.get('url') if isinstance(cfg,dict) else None
        if not isinstance(url,str) or urlsplit(url).scheme!='https' or not isinstance(cfg.get('server_name'),str):raise ValueError('Invalid guest endpoint override for '+str(key))
        result[key]=(url,cfg['server_name'])
    return result

def read_guests(url,private,server_name):
    """Return {'guests','checked','expires'} or {'error','checked'}; failures never look like an empty host."""
    now=time.time()
    try:
        with connect(url,private,server_name,accept='application/json') as response:
            if response.headers.get_content_type()!='application/json':raise ValueError('Unexpected guest inventory response')
            raw=response.read(MAX_REPORT+1)
        if len(raw)>MAX_REPORT:raise ValueError('Guest inventory too large')
        value=json.loads(raw)
        status=value.get('status') if isinstance(value,dict) else None
        if status not in ('available','unavailable','waiting'):raise ValueError('Invalid guest inventory status')
        if status!='available':
            warnings=value.get('warnings') or ['Guest inventory not collected yet']
            return {'error':str(warnings[0])[:200],'checked':now}
        checked,expires,guests=value.get('checked'),value.get('expires'),value.get('guests')
        if not all(isinstance(x,(int,float)) and math.isfinite(x) for x in (checked,expires)) or not isinstance(guests,list):raise ValueError('Invalid guest inventory')
        states={}
        for g in guests:
            if not isinstance(g,dict) or not isinstance(g.get('id'),str) or not isinstance(g.get('state'),str):raise ValueError('Invalid guest entry')
            if g['id'] in states:raise ValueError('Duplicate guest identity')
            states[g['id']]=g['state']
        return {'guests':states,'entries':[{k:g[k] for k in ('id','name','type','state','template') if k in g} for g in guests],'checked':checked,'expires':expires}
    except urllib.error.HTTPError as e:
        logging.warning('Guest inventory HTTP %s from %s',e.code,url)
        return {'error':'Guest inventory not configured on agent' if e.code==404 else 'Guest inventory unavailable','checked':now}
    except (OSError,ValueError) as e:
        logging.warning('Guest inventory unavailable from %s: %s',url,e)
        return {'error':'Guest inventory unavailable','checked':now}

def overlay(records,states,now=None):
    now=time.time() if now is None else now
    result=copy.deepcopy(records)
    for record in result:
        s=record['spec'];r=s.get('resources',{})
        if r.get('kind') not in ('machine','virtual-machine','container') and 'machine' not in s.get('capabilities',[]):continue
        parent,gid=guest_identity(s)
        if not parent or not gid:continue
        sample=states.get(parent,{})
        checked=sample.get('checked') or 0
        expires=min(sample.get('expires',checked+MAX_AGE),checked+MAX_AGE)
        fresh=checked and checked<=now+5 and now<expires
        state=sample.get('guests',{}).get(gid) if fresh and not sample.get('error') else None
        r['runtime']={'state':state if state in ('running','stopped') else 'unknown','checked':sample.get('checked'),'expires':expires,'evidence':'Authenticated metrics agent guest inventory' if state else 'Guest inventory unavailable, expired or guest missing'}
    return result

class RuntimeMonitor:
    INTERVAL=30
    def __init__(self,private):
        self.private=private;self.lock=threading.Lock();self.states={};self.started=0;self.running=False
    def refresh(self,endpoints,force=False):
        """endpoints: machine id -> (metrics stream URL, TLS identity) from the registry."""
        with self.lock:
            if self.running or (not force and time.time()-self.started<self.INTERVAL):return
            self.running=True;self.started=time.time()
        threading.Thread(target=self._read,args=(dict(endpoints),),daemon=True).start()
    def _read(self,endpoints):
        try:
            with concurrent.futures.ThreadPoolExecutor(max_workers=3) as pool:
                found=list(pool.map(lambda item:(item[0],read_guests(guests_url(item[1][0]),self.private,item[1][1])),endpoints.items()))
            with self.lock:self.states=dict(found)
        except Exception:logging.exception('Guest runtime monitoring failed')
        finally:
            with self.lock:self.running=False
    def apply(self,records):
        with self.lock:return overlay(records,self.states)

    def snapshot(self):
        with self.lock:return copy.deepcopy(self.states)

class InventorySubscriptions:
    """One persistent stream per host. No periodic /guests GET or silent fallback."""
    def __init__(self, private, changed):
        self.private=private;self.changed=changed;self.lock=threading.Lock()
        self.states={};self.workers={}
    def snapshot(self):
        with self.lock:return copy.deepcopy(self.states)
    def apply(self, records):return overlay(records,self.snapshot())
    def configure(self, endpoints):
        with self.lock:
            for key,(endpoint,stop) in list(self.workers.items()):
                if endpoints.get(key)!=endpoint:stop.set();del self.workers[key];self.states.pop(key,None)
            for key,endpoint in endpoints.items():
                if key not in self.workers:
                    stop=threading.Event();self.workers[key]=(endpoint,stop)
                    threading.Thread(target=self._stream,args=(key,endpoint,stop),daemon=True).start()
    def reconnect(self):
        # Manual verification reconnects to obtain each collector's current cache.
        with self.lock:
            endpoints={key:item[0] for key,item in self.workers.items()}
            for _,stop in self.workers.values():stop.set()
            self.workers={};self.states={}
        self.configure(endpoints)
    def _publish(self,key,stop,value):
        with self.lock:
            if stop.is_set():return
            self.states[key]=value
        self.changed.set()
    def _stream(self,key,endpoint,stop):
        retry=1
        while not stop.is_set():
            try:
                with connect(guests_url(endpoint[0])+'/stream',self.private,endpoint[1],timeout=75) as response:
                    if response.headers.get_content_type()!='application/x-ndjson':raise ValueError('Invalid inventory stream type')
                    while not stop.is_set():
                        raw=response.readline(MAX_REPORT+1)
                        if not raw or len(raw)>MAX_REPORT:raise ValueError('Inventory stream ended or frame exceeded limit')
                        value=json.loads(raw)
                        sample=parse_report(value)
                        self._publish(key,stop,sample);retry=1
            except urllib.error.HTTPError as error:
                self._publish(key,stop,{'error':'Inventory stream unsupported or unauthorized; collector upgrade required','checked':time.time()})
                if error.code in (401,403,404,405):
                    stop.wait();return
                if stop.wait(retry):return
                retry=min(300,retry*2)
                continue
            except (OSError,ValueError,HTTPException) as error:
                logging.warning('Inventory stream %s unavailable: %s',key,error)
                self._publish(key,stop,{'error':'Inventory stream unavailable; collector upgrade or recovery required','checked':time.time()})
            if stop.wait(retry):return
            retry=min(300,retry*2)

def parse_report(value):
    if not isinstance(value,dict):raise ValueError('Invalid guest report')
    hardware = parse_hardware(value.get('hardware'))
    if value.get('status') in ('waiting','unavailable'):
        return {'error':str((value.get('warnings') or ['Collector waiting'])[0])[:200],
                'checked':time.time(),'hardware':hardware}
    if value.get('status')!='available':raise ValueError('Invalid guest status')
    checked,expires=value.get('checked'),value.get('expires')
    if not all(isinstance(x,(int,float)) and math.isfinite(x) for x in (checked,expires)):raise ValueError('Invalid report timestamps')
    if checked>time.time()+5 or expires<checked:raise ValueError('Invalid observation lifetime')
    if not isinstance(value.get('guests'),list):raise ValueError('Missing complete guest list')
    entries=[];states={}
    for g in value['guests']:
        if not isinstance(g,dict) or not isinstance(g.get('id'),str) or not isinstance(g.get('state'),str) or g['id'] in states:raise ValueError('Invalid guest identity')
        states[g['id']]=g['state'];entries.append({k:g[k] for k in ('id','name','type','state','template') if k in g})
    return {'checked':checked,'expires':expires,'entries':entries,'guests':states,'hardware':hardware}

def parse_hardware(value):
    if not isinstance(value, dict):
        return None
    system, arch = value.get('os'), value.get('architecture')
    count, memory, checked = value.get('logical_cpus'), value.get('memory_bytes'), value.get('checked')
    if (system not in ('darwin', 'linux', 'windows', 'freebsd')
            or arch not in ('amd64', 'arm64', '386', 'arm', 'riscv64', 'ppc64le', 's390x')
            or type(count) is not int or not 0 < count <= 65536
            or type(memory) is not int or not 0 < memory <= 1 << 60
            or type(checked) not in (int, float) or not 0 < checked <= time.time() + 5):
        return None
    return dict(os=system, architecture=arch, logical_cpus=count, memory_bytes=memory, checked=checked)
