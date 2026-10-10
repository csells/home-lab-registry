"""Demand-driven fan-out of cached, mutually authenticated TLS metrics streams."""
import contextlib,json,logging,math,ssl,threading,time
from collections import deque
from http.client import HTTPException
from urllib.parse import urlsplit
from urllib.request import build_opener,HTTPSHandler,HTTPRedirectHandler,ProxyHandler,Request

LOG=logging.getLogger(__name__)
MAX_FRAME=262144
class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self,*args,**kwargs):return None

def connect(url,private):
    ctx=ssl.create_default_context(cafile=str(private/'stream-ca.pem'))
    ctx.minimum_version=ssl.TLSVersion.TLSv1_3
    ctx.load_cert_chain(str(private/'stream-client.pem'),str(private/'stream-client-key.pem'))
    opener=build_opener(ProxyHandler({}),HTTPSHandler(context=ctx),NoRedirect())
    return opener.open(Request(url,headers={'Accept':'application/x-ndjson'}),timeout=15)

class Stream:
    def __init__(self,url,private):
        self.url=url;self.private=private;self.cv=threading.Condition();self.stop=threading.Event()
        self.history=deque(maxlen=120);self.version=0;self.viewers=0
        self.error='Connecting to collector';self.worker=threading.Thread(target=self.run,daemon=True)
    def start(self):self.worker.start()
    def close(self):
        self.stop.set()
        with self.cv:self.cv.notify_all()
    def reading(self):
        latest=dict(self.history[-1]) if self.history else {'status':'waiting'}
        if self.error:latest.update(status='unavailable',error=self.error)
        elif self.history and latest.get('expires',0)<time.time():latest.update(status='unavailable',error='Collector data expired')
        return {**latest,'history':list(self.history),'interval_seconds':5}
    def receive(self,frame):
        if not isinstance(frame,dict) or frame.get('type') not in ('history','samples','heartbeat'):raise ValueError('Invalid collector frame')
        if frame.get('type')=='heartbeat':return
        key='history' if frame['type']=='history' else 'samples'
        samples=frame.get(key)
        if not isinstance(samples,list) or len(samples)>120:raise ValueError('Invalid collector frame')
        for sample in samples:
            if not isinstance(sample,dict) or not isinstance(sample.get('checked'),(int,float)) or not math.isfinite(sample['checked']) or sample.get('status') not in ('available','unavailable'):raise ValueError('Invalid collector sample')
        with self.cv:
            if key=='history':self.history.clear()
            for sample in samples:
                if not self.history or sample['checked']>self.history[-1]['checked']:self.history.append(sample)
            self.error=None;self.version+=1;self.cv.notify_all()
    def run(self):
        retry=1
        while not self.stop.is_set():
            try:
                with connect(self.url,self.private) as response:
                    if response.headers.get_content_type()!='application/x-ndjson':raise ValueError('Unexpected collector response')
                    while not self.stop.is_set():
                        raw=response.readline(MAX_FRAME+1)
                        if not raw:raise OSError('Collector stream closed')
                        if len(raw)>MAX_FRAME:raise ValueError('Collector frame too large')
                        self.receive(json.loads(raw));retry=1
            except (OSError,ValueError,HTTPException):
                if not self.stop.is_set():
                    LOG.warning('Metrics stream disconnected; retrying while subscribed')
                    with self.cv:self.error='Collector stream unavailable';self.version+=1;self.cv.notify_all()
            if self.stop.wait(retry):return
            retry=min(30,retry*2)

class LiveMetrics:
    def __init__(self,private):
        self.private=private;self.lock=threading.Lock();self.targets={};self.streams={}
    def configure(self,records):
        targets={}
        for record in records:
            s=record['spec'];key=record['id'];cfg=s.get('resources',{}).get('metrics_stream',{})
            if s.get('lifecycle')!='active' or not cfg.get('enabled'):continue
            try:
                endpoint=cfg.get('url','');url=urlsplit(endpoint)
                if url.scheme!='https' or not url.hostname or url.username or url.password or url.path!='/stream' or url.query or url.fragment or not url.port:continue
            except ValueError:continue
            targets[key]=endpoint
        with self.lock:
            for key,state in list(self.streams.items()):
                if targets.get(key)!=self.targets.get(key):state.close();del self.streams[key]
            self.targets=targets
    def readings(self):
        with self.lock:
            result={key:{'status':'waiting'} for key in self.targets}
            for key,s in self.streams.items():
                with s.cv:result[key]=s.reading()
            return result
    @contextlib.contextmanager
    def subscribe(self,key):
        with self.lock:
            if key not in self.targets:raise KeyError(key)
            state=self.streams.get(key)
            if state is None:
                state=Stream(self.targets[key],self.private);self.streams[key]=state;state.start()
                LOG.info('Metrics upstream opened: %s',key)
            state.viewers+=1
        try:yield state
        finally:
            with self.lock:
                state.viewers-=1
                if state.viewers==0:
                    state.close()
                    if self.streams.get(key) is state:del self.streams[key]
                    LOG.info('Metrics upstream closed, no viewers: %s',key)
