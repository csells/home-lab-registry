import base64,hmac,json,pathlib,threading,time,logging
from http.server import ThreadingHTTPServer,BaseHTTPRequestHandler
from client import request
from model import inventory,address_audit_summary
from reconciliation import reconcile as reconcile_inventory
from metrics import LiveMetrics
from urllib.parse import urlsplit,parse_qs
from access import credential
from guac import launch
from machine_runtime import InventorySubscriptions,guest_endpoints
BASE=pathlib.Path(__file__).parent;PRIVATE=BASE/'private';LOCK=threading.Lock();STATE={'machines':[],'services':[],'backups':[],'error':'Loading registry','fetched_at':None};LIVE=None;WAKE=threading.Event();RUNTIME=InventorySubscriptions(PRIVATE,WAKE);LAST_REQUEST=0;CATALOG_REFRESH=threading.Event();UPDATES=threading.Condition();VERSION=0
def reconcile():
    global STATE,VERSION
    cfg=json.loads((PRIVATE/'reader.json').read_text())
    records=None
    while True:
        WAKE.clear()
        try:
            if records is None or CATALOG_REFRESH.is_set():
                CATALOG_REFRESH.clear()
                records=request(cfg,'GET','/v1/services')['services']
                LIVE.configure(records)
                RUNTIME.configure(guest_endpoints(LIVE.endpoints(),json.loads((PRIVATE/'guest-endpoints.json').read_text()) if (PRIVATE/'guest-endpoints.json').exists() else None))
            states=RUNTIME.snapshot();accuracy=reconcile_inventory(records,states)
            accuracy['pending']=any(key not in states for key in LIVE.endpoints())
            value=inventory(RUNTIME.apply(records),LIVE.readings())
            value['inventory_accuracy']=accuracy;value['address_audit']=address_audit_summary(records);value['fetched_at']=time.time();value['error']=None
            with LOCK:STATE=value
        except Exception:
            logging.exception('Reconciliation failed')
            with LOCK:STATE={**STATE,'error':'Registry refresh failed; displayed readings may be stale.'}
        with UPDATES:VERSION+=1;UPDATES.notify_all()
        # Timeout only expires cached observations. It performs no network discovery.
        WAKE.wait(90)
class Handler(BaseHTTPRequestHandler):
    def log_message(self,*args):pass
    def do_POST(self):
        global LAST_REQUEST
        if not hmac.compare_digest(self.headers.get('Authorization',''),self.server.auth):
            self.send_error(401);return
        # Reject cross-origin requests and form submissions before reading any secret.
        allowed=json.loads((PRIVATE/'origins.json').read_text())
        action={'/api/copy-password':'copy-password','/api/reconcile':'reconcile'}.get(self.path)
        if not action or self.headers.get('Origin') not in allowed or self.headers.get('X-Lab-Action')!=action or self.headers.get('Content-Type')!='application/json':
            self.send_error(403);return
        if self.path=='/api/reconcile':
            with LOCK:
                if time.time()-LAST_REQUEST<10:self.send_error(429,'Reconciliation already requested');return
                LAST_REQUEST=time.time()
            CATALOG_REFRESH.set();RUNTIME.reconnect();WAKE.set()
            self.send_response(202);self.send_header('Content-Length','0');self.send_header('Cache-Control','no-store');self.end_headers();return
        if self.path!='/api/copy-password':self.send_error(404);return
        try:
            length=int(self.headers.get('Content-Length','0'))
            if not 0<length<=1024:raise ValueError()
            body=json.loads(self.rfile.read(length))
            if not isinstance(body.get('machine'),str) or not isinstance(body.get('login'),str):raise ValueError()
            value=credential(PRIVATE,body['machine'],body['login'])
        except (ValueError,TypeError,KeyError):self.send_error(400);return
        if value is None:self.send_error(404);return
        raw=value.encode()
        self.send_response(200);self.send_header('Content-Type','text/plain; charset=utf-8');self.send_header('Content-Length',str(len(raw)));self.send_header('Cache-Control','no-store, private');self.send_header('Pragma','no-cache');self.send_header('X-Content-Type-Options','nosniff');self.end_headers();self.wfile.write(raw)
    def do_GET(self):
        if not hmac.compare_digest(self.headers.get('Authorization',''),self.server.auth):
            self.send_response(401);self.send_header('WWW-Authenticate','Basic realm="Home lab preview"');self.send_header('Content-Length','0');self.end_headers();return
        path=self.path.split('?')[0]
        if path=='/api/inventory/stream':
            allowed=json.loads((PRIVATE/'origins.json').read_text())
            if self.headers.get('Sec-Fetch-Site')=='cross-site' or (self.headers.get('Origin') and self.headers['Origin'] not in allowed):self.send_error(403);return
            self.send_response(200);self.send_header('Content-Type','text/event-stream');self.send_header('Cache-Control','no-store');self.send_header('X-Accel-Buffering','no');self.end_headers()
            self.connection.settimeout(10);self.close_connection=True;version=-1
            try:
                while True:
                    with UPDATES:
                        if version==VERSION:UPDATES.wait(60)
                        version=VERSION
                    with LOCK:payload={**STATE,'server_time':time.time()}
                    self.wfile.write(('event: inventory\ndata: '+json.dumps(payload)+'\n\n').encode());self.wfile.flush()
            except (OSError,TimeoutError):pass
            return
        if path.startswith('/terminal/'):
            if self.headers.get('Sec-Fetch-Site')=='cross-site':self.send_error(403);return
            try: destination=launch(PRIVATE,path.removeprefix('/terminal/').rstrip('/'))
            except Exception:
                self.send_error(503,'Terminal unavailable; verify registry and pinned access');return
            self.send_response(302);self.send_header('Location',destination);self.send_header('Cache-Control','no-store, private');self.send_header('Referrer-Policy','no-referrer');self.send_header('Content-Length','0');self.end_headers();return
        if path=='/api/metrics/stream':
            allowed=json.loads((PRIVATE/'origins.json').read_text())
            if self.headers.get('Sec-Fetch-Site')=='cross-site' or (self.headers.get('Origin') and self.headers['Origin'] not in allowed):self.send_error(403);return
            key=parse_qs(urlsplit(self.path).query).get('machine',[''])[0]
            try:
                with LIVE.subscribe(key) as stream:
                    self.send_response(200);self.send_header('Content-Type','text/event-stream');self.send_header('Cache-Control','no-store');self.send_header('X-Accel-Buffering','no');self.send_header('Connection','close');self.end_headers()
                    self.connection.settimeout(10);self.close_connection=True;version=-1
                    while not stream.stop.is_set():
                        with stream.cv:
                            if stream.version==version:stream.cv.wait(5)
                            if stream.version!=version:
                                value=stream.reading();version=stream.version
                                raw=('event: metrics\ndata: '+json.dumps(value)+'\n\n').encode()
                            else:raw=b': keepalive\n\n'
                        self.wfile.write(raw);self.wfile.flush()
            except KeyError:self.send_error(404)
            except (BrokenPipeError,ConnectionResetError,TimeoutError):pass
            return
        elif path in ['/api/inventory','/api/machines','/api/services','/api/backups','/api/reconciliation']:
            with LOCK:data=dict(STATE)
            data['server_time']=time.time()
            if path=='/api/reconciliation':data={'inventory_accuracy':data.get('inventory_accuracy'),'error':data['error'],'fetched_at':data['fetched_at']}
            elif path!='/api/inventory':key=path.rsplit('/',1)[1];data={key:data[key],'error':data['error'],'fetched_at':data['fetched_at']}
            raw=json.dumps(data).encode();kind='application/json'
        elif path in ['/','/app.js','/app.css','/resource-graph.js','/resource-graph.css','/vendor/elk-api.js','/vendor/elk-worker.min.js']:
            file='index.html' if path=='/' else path.lstrip('/');raw=(BASE/file).read_bytes();kind='text/html' if path=='/' else 'text/css' if path.endswith('.css') else 'text/javascript'
        else:self.send_error(404);return
        self.send_response(200);self.send_header('Content-Type',kind+'; charset=utf-8');self.send_header('Content-Length',str(len(raw)));self.send_header('Cache-Control','no-store');self.send_header('X-Content-Type-Options','nosniff');self.send_header('Content-Security-Policy',"default-src 'self'; style-src 'self'; script-src 'self'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'");self.end_headers();self.wfile.write(raw)
if __name__=='__main__':
    LIVE=LiveMetrics(PRIVATE)
    logging.basicConfig(level=logging.INFO);credentials=json.loads((PRIVATE/'login.json').read_text());s=ThreadingHTTPServer(('127.0.0.1',8089),Handler);s.auth='Basic '+base64.b64encode((credentials['username']+':'+credentials['password']).encode()).decode();threading.Thread(target=reconcile,daemon=True).start();s.serve_forever()
