import base64,hmac,json,pathlib,threading,time,logging
from http.server import ThreadingHTTPServer,BaseHTTPRequestHandler
from http.client import HTTPException
from catalog_updates import subscribe_catalog
from client import request
from model import inventory,address_audit_summary
from reconciliation import reconcile as reconcile_inventory
from metrics import LiveMetrics
from urllib.parse import urlsplit,parse_qs,urlencode
from uuid import uuid4
from lease_review import LeaseInventory, LeaseIssueFeed, page as lease_review_page
from access import credential, scoped_records
from guac import launch
from machine_runtime import InventorySubscriptions,guest_endpoints,overlay
BASE=pathlib.Path(__file__).parent;PRIVATE=BASE/'private';LOCK=threading.Lock();STATE={'machines':[],'services':[],'backups':[],'error':'Loading registry','fetched_at':None};LIVE=None;WAKE=threading.Event();RUNTIME=InventorySubscriptions(PRIVATE,WAKE);LAST_REQUEST=0;CATALOG_REFRESH=threading.Event();UPDATES=threading.Condition();VERSION=0;MANUAL=None;CATALOG_FEED='waiting'
LEASE_ISSUES = LeaseIssueFeed(PRIVATE / 'lease-review.json', WAKE)

def reconcile():
    global STATE,VERSION,MANUAL,CATALOG_FEED
    cfg=json.loads((PRIVATE/'reader.json').read_text())
    leases=LeaseInventory(PRIVATE/'lease-review.json')
    records=None
    retry_at = 0
    retry_delay = 1
    running_comparison = None
    catalog_checked = None
    source_snapshot = cfg.get('catalog_snapshot_at')
    affected_health = set()
    if cfg.get('catalog_change_stream', True):
        def invalidate():
            CATALOG_REFRESH.set()
            WAKE.set()
        def catalog_connection(connected):
            global CATALOG_FEED
            with LOCK:
                CATALOG_FEED = 'connected' if connected else 'disconnected'
            if connected:
                CATALOG_REFRESH.set()
            WAKE.set()
        def update_health(ids):
            with LOCK:
                affected_health.update(ids)
            WAKE.set()
        threading.Thread(target=subscribe_catalog, args=(cfg, invalidate, catalog_connection),
                         kwargs={'update_health':update_health}, daemon=True).start()
    else:
        CATALOG_FEED = 'disabled'
    while True:
        WAKE.clear()
        with LOCK:
            queued = MANUAL if MANUAL and MANUAL['state'] == 'pending' else None
        comparison_start = queued and queued['id'] != running_comparison
        refresh = records is None or CATALOG_REFRESH.is_set() or comparison_start
        if refresh and retry_at > time.monotonic():
            WAKE.wait(max(0, retry_at - time.monotonic()))
            continue
        with LOCK:
            health_ids = sorted(affected_health)
            affected_health.clear()
        try:
            if refresh:
                CATALOG_REFRESH.clear()
                try:
                    current = scoped_records(request(cfg,'GET','/v1/services')['services'])
                    LIVE.configure(current)
                    RUNTIME.configure(guest_endpoints(LIVE.endpoints(),json.loads((PRIVATE/'guest-endpoints.json').read_text()) if (PRIVATE/'guest-endpoints.json').exists() else None))
                except Exception:
                    CATALOG_REFRESH.set()
                    retry_at = time.monotonic() + retry_delay
                    retry_delay = min(retry_delay * 2, 30)
                    raise
                records = current
                catalog_checked = time.time()
                retry_at = 0
                retry_delay = 1
            elif health_ids:
                try:
                    observations = {}
                    for start in range(0,len(health_ids),128):
                        query = urlencode([('id',key) for key in health_ids[start:start+128]])
                        observations.update(request(cfg,'GET','/v1/health?'+query)['health'])
                    requested = set(health_ids)
                    records = [{**row,'health':observations.get(row['id'],{'status':'unknown'})}
                               if row['id'] in requested else row for row in records]
                except Exception:
                    CATALOG_REFRESH.set()
                    WAKE.set()
                    raise
            if comparison_start:
                RUNTIME.reconnect()
                running_comparison = queued['id']
            states=RUNTIME.snapshot()
            claims,unverified=leases.reconcile(states,force=bool(comparison_start))
            accuracy=reconcile_inventory(records,states,lease_claims=claims,lease_unverified=unverified)
            accuracy['pending']=any(key not in states for key in LIVE.endpoints())
            # Cards and comparison use exactly the same collector snapshot.
            value=inventory(overlay(records,states),LIVE.readings())
            value['inventory_accuracy']=accuracy;value['address_audit']=address_audit_summary(records);value['fetched_at']=time.time();value['error']=None
            with LOCK:
                if queued and (not accuracy['pending'] or time.monotonic()>=queued['deadline']):
                    MANUAL={**queued,'state':'complete'}
                value['comparison']=public_comparison(MANUAL)
                value['catalog']={'checked':catalog_checked,'feed':CATALOG_FEED,'snapshot_at':source_snapshot}
                value['lease_issues']=LEASE_ISSUES.snapshot()
                STATE=value
        except Exception:
            logging.exception('Reconciliation failed')
            with LOCK:
                if queued and time.monotonic()>=queued['deadline']:
                    MANUAL={**queued,'state':'failed'}
                STATE={**STATE,'error':'Registry refresh failed; displayed readings may be stale.',
                       'comparison':public_comparison(MANUAL),
                       'catalog':{'checked':catalog_checked,'feed':CATALOG_FEED,'snapshot_at':source_snapshot}}
        with UPDATES:VERSION+=1;UPDATES.notify_all()
        # Ordinary idle wakes age cached observations. Explicit failures have their own retries.
        wait = max(0, retry_at - time.monotonic()) if retry_at else 90
        if leases.retry_at is not None:
            wait=min(wait,max(0,leases.retry_at-time.time()))
        with LOCK:
            if MANUAL and MANUAL['state']=='pending':
                wait=min(wait,max(0,MANUAL['deadline']-time.monotonic()))
        WAKE.wait(wait)

def public_comparison(comparison):
    return {key:comparison[key] for key in ('id','state')} if comparison else None

class Handler(BaseHTTPRequestHandler):
    def log_message(self,*args):pass
    def do_POST(self):
        global LAST_REQUEST,MANUAL
        if not hmac.compare_digest(self.headers.get('Authorization',''),self.server.auth):
            self.send_error(401);return
        # Reject cross-origin requests and form submissions before reading any secret.
        allowed=json.loads((PRIVATE/'origins.json').read_text())
        action={'/api/copy-password':'copy-password','/api/reconcile':'reconcile','/api/refresh':'refresh-catalog'}.get(self.path)
        if not action or self.headers.get('Origin') not in allowed or self.headers.get('X-Lab-Action')!=action or self.headers.get('Content-Type')!='application/json':
            self.send_error(403);return
        if self.path=='/api/refresh':
            accepted = time.time()
            deadline = time.monotonic() + 12
            LEASE_ISSUES.refresh()
            CATALOG_REFRESH.set()
            WAKE.set()
            with UPDATES:
                while True:
                    with LOCK:
                        snapshot = dict(STATE)
                    checked = (snapshot.get('catalog') or {}).get('checked')
                    if checked and checked >= accepted and not snapshot['error']:
                        break
                    remaining = deadline - time.monotonic()
                    if remaining <= 0:
                        self.send_error(503,'Authoritative catalog refresh did not complete')
                        return
                    UPDATES.wait(remaining)
            snapshot.update(server_time=time.time(),lease_review_enabled=LEASE_ISSUES.enabled)
            raw = json.dumps(snapshot).encode()
            self.send_response(200)
            self.send_header('Content-Type','application/json')
            self.send_header('Content-Length',str(len(raw)))
            self.send_header('Cache-Control','no-store')
            self.end_headers()
            self.wfile.write(raw)
            return
        if self.path=='/api/reconcile':
            with LOCK:
                now=time.monotonic()
                if (MANUAL and MANUAL['state']=='pending') or (LAST_REQUEST and now-LAST_REQUEST<10):
                    self.send_error(429,'Reconciliation already requested');return
                LAST_REQUEST=now
                MANUAL={'id':uuid4().hex,'state':'pending','deadline':now+45}
                raw=json.dumps(public_comparison(MANUAL)).encode()
            LEASE_ISSUES.refresh()
            WAKE.set()
            self.send_response(202);self.send_header('Content-Type','application/json');self.send_header('Content-Length',str(len(raw)));self.send_header('Cache-Control','no-store');self.end_headers();self.wfile.write(raw);return
        if self.path!='/api/copy-password':self.send_error(404);return
        try:
            length=int(self.headers.get('Content-Length','0'))
            if not 0<length<=1024:raise ValueError()
            body=json.loads(self.rfile.read(length))
            if not isinstance(body.get('machine'),str) or not isinstance(body.get('login'),str):raise ValueError()
            with LOCK: machines=STATE['machines']
            value=credential(PRIVATE,body['machine'],body['login'],machines)
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
            with LEASE_ISSUES.watch():
                try:
                    while True:
                        with UPDATES:
                            if version==VERSION:UPDATES.wait(15)
                            changed=version!=VERSION
                            version=VERSION
                        if not changed:
                            self.wfile.write(b': keepalive\n\n');self.wfile.flush()
                            continue
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
        elif path=='/api/leases':
            config=PRIVATE/'lease-review.json'
            if not config.exists():self.send_error(404);return
            try:
                query=parse_qs(urlsplit(self.path).query,keep_blank_values=True)
                cursor,=query.pop('before',[None])
                scope,=query.pop('scope',['all'])
                if scope not in ('all','current','released','counts'):raise ValueError()
                if scope=='counts' and cursor is not None:raise ValueError()
                if query or (cursor is not None and (not cursor.isascii() or not cursor.isdigit() or len(cursor)>18 or cursor.startswith('0'))):raise ValueError()
            except ValueError:self.send_error(400,'Invalid lease query');return
            try:raw=json.dumps(lease_review_page(config,cursor,scope)).encode()
            except (OSError,HTTPException,ValueError,KeyError):self.send_error(503,'Lease history unavailable');return
            kind='application/json'
        elif path in ['/api/inventory','/api/machines','/api/services','/api/backups','/api/reconciliation']:
            with LOCK:data=dict(STATE)
            data['server_time']=time.time()
            data['lease_review_enabled']=(PRIVATE/'lease-review.json').is_file()
            if path=='/api/reconciliation':
                data={key:data.get(key) for key in ('inventory_accuracy','error','fetched_at','comparison')}
            elif path!='/api/inventory':key=path.rsplit('/',1)[1];data={key:data[key],'error':data['error'],'fetched_at':data['fetched_at']}
            raw=json.dumps(data).encode();kind='application/json'
        elif path in ['/','/leases','/leases.js','/leases.css','/app.js','/dashboard-react.js','/app.css','/resource-graph.js','/resource-graph.css','/vendor/elk-api.js','/vendor/elk-worker.min.js']:
            file='index.html' if path in ('/','/leases') else path.lstrip('/');raw=(BASE/file).read_bytes();kind='text/html' if path in ('/','/leases') else 'text/css' if path.endswith('.css') else 'text/javascript'
        else:self.send_error(404);return
        self.send_response(200);self.send_header('Content-Type',kind+'; charset=utf-8');self.send_header('Content-Length',str(len(raw)));self.send_header('Cache-Control','no-store');self.send_header('X-Content-Type-Options','nosniff');self.send_header('Content-Security-Policy',"default-src 'self'; style-src 'self'; script-src 'self'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'");self.end_headers();self.wfile.write(raw)
if __name__=='__main__':
    LIVE=LiveMetrics(PRIVATE)
    threading.Thread(target=LEASE_ISSUES.run,daemon=True).start()
    logging.basicConfig(level=logging.INFO);credentials=json.loads((PRIVATE/'login.json').read_text());s=ThreadingHTTPServer(('127.0.0.1',8089),Handler);s.auth='Basic '+base64.b64encode((credentials['username']+':'+credentials['password']).encode()).decode();threading.Thread(target=reconcile,daemon=True).start();s.serve_forever()
