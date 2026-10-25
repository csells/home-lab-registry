"""Guest power state read from each machine's mutually authenticated metrics agent."""
import concurrent.futures,copy,json,logging,math,threading,time,urllib.error
from urllib.parse import urlsplit,urlunsplit
from metrics import connect
from network_audit import guest_identity

MAX_REPORT=4<<20
MAX_AGE=900  # Never trust a guest state older than this, whatever the agent claims.

def guests_url(stream_url):
    u=urlsplit(stream_url)
    return urlunsplit((u.scheme,u.netloc,'/guests','',''))

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
            states[g['id']]=g['state']
        return {'guests':states,'checked':checked,'expires':expires}
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
        fresh=checked and now<expires
        state=sample.get('guests',{}).get(gid) if fresh and not sample.get('error') else None
        r['runtime']={'state':state if state in ('running','stopped') else 'unknown','checked':sample.get('checked'),'expires':expires,'evidence':'Authenticated metrics agent guest inventory' if state else 'Guest inventory unavailable, expired or guest missing'}
    return result

class RuntimeMonitor:
    INTERVAL=60
    def __init__(self,private):
        self.private=private;self.lock=threading.Lock();self.states={};self.started=0;self.running=False
    def refresh(self,endpoints):
        """endpoints: machine id -> (metrics stream URL, TLS identity) from the registry."""
        with self.lock:
            if self.running or time.time()-self.started<self.INTERVAL:return
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
