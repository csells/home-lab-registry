#!/usr/bin/env python3
"""Private service catalog. TLS and read/write bearer credentials are required in production."""
from contextlib import contextmanager
import presence
import argparse, concurrent.futures, hmac, json, logging, re, sqlite3, ssl, threading, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlsplit, parse_qs
from urllib.request import Request, build_opener, HTTPRedirectHandler

LOG = logging.getLogger('registry')
ID = re.compile(r'^[a-z][a-z0-9-]{0,63}$')
FIELDS = {'name','host','owner','capabilities','endpoints','lifecycle','runbook','notes','dependencies','credential_ref','deployment','resources','backup','config_backup','probe'}
class Problem(Exception):
    def __init__(self, status, message): self.status, self.message = status, message

def validate_url(url):
    if not isinstance(url,str): raise Problem(400,'URL must be a string')
    u=urlsplit(url)
    if u.scheme not in ('http','https','ssh') or not u.hostname or u.username or u.password or u.query or u.fragment:
        raise Problem(400,'Use http/https/ssh URLs without embedded credentials, query or fragment')

def validate(d):
    if not isinstance(d,dict) or set(d)-FIELDS: raise Problem(400,'Unknown fields; store credential references, never secrets')
    for f in ('name','host','owner','runbook'):
        if not isinstance(d.get(f),str) or not d[f].strip(): raise Problem(400,'Required nonempty string: '+f)
    for f in ('capabilities','dependencies'):
        if not isinstance(d.get(f),list) or not all(isinstance(v,str) and v for v in d[f]): raise Problem(400,'Required string array: '+f)
    if d.get('lifecycle') not in ('active','planned','retired'): raise Problem(400,'Invalid lifecycle')
    if not isinstance(d.get('endpoints'),dict): raise Problem(400,'endpoints must be an object')
    for u in d['endpoints'].values(): validate_url(u)
    for f in ('notes','credential_ref','deployment','backup'):
        if f in d and not isinstance(d[f],str): raise Problem(400,f+' must be a string')
    if 'config_backup' in d:
        b=d['config_backup']
        if not isinstance(b,dict) or set(b)-{'status','locations','notes'} or b.get('status') not in ('unknown','manual','scheduled'):
            raise Problem(400,'Invalid configuration backup metadata')
        locations=b.get('locations')
        if not isinstance(locations,list) or (b['status']!='unknown' and not locations):
            raise Problem(400,'Configuration backup locations required')
        for location in locations:
            if not isinstance(location,dict) or set(location)!={'host','path'} or not isinstance(location['host'],str) or not ID.fullmatch(location['host']) or not isinstance(location['path'],str) or not location['path'].startswith('/') or any(ord(c)<32 for c in location['path']):
                raise Problem(400,'Use a machine name and absolute backup path, never credentials')
        if 'notes' in b and not isinstance(b['notes'],str): raise Problem(400,'Backup notes must be a string')
    if 'resources' in d and not isinstance(d['resources'],dict): raise Problem(400,'resources must be an object')
    resources=d.get('resources',{})
    try:
        if 'registration' in resources:presence.validate_policy(resources['registration'])
        bindings=resources.get('address_bindings',[])
        if not isinstance(bindings,list) or len(bindings)>32:raise ValueError()
        for binding in bindings:presence.validate_policy(binding,True)
    except ValueError:raise Problem(400,'Invalid address bindings')
    if 'probe' in d:
        p=d['probe']
        if isinstance(p,dict) and 'tcp_host' in p:
            if set(p)-{'tcp_host','tcp_port','vantage'} or not isinstance(p['tcp_host'],str) or not p['tcp_host'].strip() or any(c in p['tcp_host'] for c in '/@?#'):
                raise Problem(400,'Invalid TCP host')
            if type(p.get('tcp_port')) is not int or not 1<=p['tcp_port']<=65535 or p.get('vantage','registry') not in ('desktop-node','registry'):
                raise Problem(400,'Invalid TCP probe')
            return d
        if isinstance(p,dict) and 'check' in p:
            from local_checks import CHECKS
            if set(p)!={'check','vantage'} or not isinstance(p.get('vantage'),str) or not ID.fullmatch(p.get('vantage','')) or not isinstance(p.get('check'),str) or p.get('check') not in CHECKS:
                raise Problem(400,'Invalid local check')
            return d
        if not isinstance(p,dict) or set(p)-{'url','expected_status','vantage','bearer_ref','json_not_in'} or 'url' not in p: raise Problem(400,'Invalid probe')
        validate_url(p['url'])
        if 'bearer_ref' in p or 'json_not_in' in p:
            if p.get('vantage','registry')!='registry': raise Problem(400,'Authenticated/JSON probes require registry vantage')
        if 'bearer_ref' in p and (not isinstance(p['bearer_ref'],str) or not ID.fullmatch(p['bearer_ref'])):
            raise Problem(400,'Invalid bearer credential reference')
        if 'json_not_in' in p:
            rules=p['json_not_in']
            if not isinstance(rules,dict) or not rules or not all(isinstance(k,str) and isinstance(v,list) and all(isinstance(x,str) for x in v) for k,v in rules.items()):
                raise Problem(400,'Invalid JSON health rules')

        if not isinstance(p.get('vantage','registry'),str) or not ID.fullmatch(p.get('vantage','registry')): raise Problem(400,'Unknown probe vantage')
        if urlsplit(p['url']).scheme not in ('http','https'): raise Problem(400,'Probe requires HTTP(S)')
        if type(p.get('expected_status',200)) is not int or not 100<=p.get('expected_status',200)<=599: raise Problem(400,'Invalid probe status')
    return d

class Catalog:
    def __init__(self,path):
        self.path=path;self.presence_verifier=None
        with self.db() as db:
            db.executescript('''PRAGMA journal_mode=WAL;
            CREATE TABLE IF NOT EXISTS services (id TEXT PRIMARY KEY, revision INTEGER NOT NULL, document TEXT NOT NULL, updated REAL NOT NULL);
            CREATE TABLE IF NOT EXISTS history (seq INTEGER PRIMARY KEY, id TEXT NOT NULL, revision INTEGER NOT NULL, document TEXT NOT NULL, actor TEXT NOT NULL, updated REAL NOT NULL);
            CREATE TABLE IF NOT EXISTS presence (id TEXT PRIMARY KEY, address TEXT NOT NULL, interface TEXT NOT NULL, checked REAL NOT NULL, expires REAL NOT NULL);
            CREATE TABLE IF NOT EXISTS health (id TEXT PRIMARY KEY, status TEXT NOT NULL, evidence TEXT NOT NULL, checked REAL NOT NULL, expires REAL NOT NULL);
            ''')
    @contextmanager
    def db(self):
        db=sqlite3.connect(self.path,timeout=10); db.row_factory=sqlite3.Row
        try:
            with db: yield db
        finally: db.close()
    def get(self,id):
        with self.db() as db:
            row=db.execute('SELECT * FROM services WHERE id=?',(id,)).fetchone()
            h=db.execute('SELECT * FROM health WHERE id=?',(id,)).fetchone()
            p=db.execute('SELECT address,interface,checked,expires FROM presence WHERE id=?',(id,)).fetchone()
        if not row: raise Problem(404,'Service not found')
        health={'status':'unknown'} if not h else dict(h)
        if h and h['expires']<time.time(): health['status']='stale'
        record=presence.overlay({'id':id,'revision':row['revision'],'updated_at':row['updated'],'spec':json.loads(row['document']),'health':health},dict(p) if p else None)
        # Service placement is explicit: the physical parent is not the guest IP.
        bindings=record['spec'].get('resources',{}).get('address_bindings',[])
        with self.db() as db:
            for binding in bindings:
                source=db.execute('SELECT p.address,p.interface,p.checked,p.expires FROM presence p JOIN services s ON p.id=s.id WHERE p.id=?',(binding['machine_id'],)).fetchone()
                if source:
                    presence.overlay(record,dict(source),binding)
        return record
    def announce(self,id,body,enrollment):
        record=self.get(id)
        if record['spec']['lifecycle']!='active' or 'machine' not in record['spec']['capabilities']:raise Problem(403,'Active enrolled machine required')
        try:body=presence.validate(body,enrollment)
        except (ValueError,TypeError,KeyError):raise Problem(400,'Invalid enrolled address announcement')
        previous=record['spec'].get('resources',{}).get('agent_presence',{})
        if previous.get('address')!=body['address'] and self.presence_verifier:
            try:self.presence_verifier(body['address'],enrollment)
            except Exception:raise Problem(409,'Collector identity not verified at announced address')
        now=time.time()
        with self.db() as db:
            db.execute('INSERT OR REPLACE INTO presence VALUES(?,?,?,?,?)',(id,body['address'],body['interface'],now,now+300))
        return {'accepted':True,'expires':now+300}
    def listing(self,query):
        with self.db() as db: ids=[r[0] for r in db.execute('SELECT id FROM services ORDER BY id')]
        result=[]
        for id in ids:
            r=self.get(id); d=r['spec']
            if d['lifecycle']=='retired' and query.get('include_retired')!=['true']: continue
            if 'capability' in query and query['capability'][0] not in d['capabilities']: continue
            if 'host' in query and query['host'][0]!=d['host']: continue
            if 'status' in query and query['status'][0]!=r['health']['status']: continue
            result.append(r)
        return result
    def put(self,id,doc,revision):
        doc=json.loads(json.dumps(doc));doc.get('resources',{}).pop('agent_presence',None)
        validate(doc); now=time.time()
        with self.db() as db:
            db.execute('BEGIN IMMEDIATE')
            r=db.execute('SELECT revision FROM services WHERE id=?',(id,)).fetchone()
            current=r[0] if r else 0
            if revision!=current: raise Problem(409,'Revision conflict; fetch latest record before updating')
            value=json.dumps(doc,sort_keys=True)
            db.execute('INSERT OR REPLACE INTO services VALUES(?,?,?,?)',(id,current+1,value,now))
            db.execute('INSERT INTO history(id,revision,document,actor,updated) VALUES(?,?,?,?,?)',(id,current+1,value,'writer',now))
            db.execute('DELETE FROM health WHERE id=?',(id,))
        return self.get(id)
    def observe(self,id,d,revision=None):
        self.get(id)
        if not isinstance(d,dict) or set(d)-{'status','evidence','ttl_seconds'}: raise Problem(400,'Invalid observation')
        if d.get('status') not in ('healthy','unhealthy','unknown'): raise Problem(400,'Invalid health status')
        if not isinstance(d.get('evidence'),str) or not d['evidence'].strip() or len(d['evidence'])>2000: raise Problem(400,'Evidence required, maximum 2000 characters')
        ttl=d.get('ttl_seconds',300)
        if type(ttl) is not int or not 1<=ttl<=86400: raise Problem(400,'TTL must be 1..86400 seconds')
        now=time.time()
        with self.db() as db:
            db.execute('BEGIN IMMEDIATE')
            current=db.execute('SELECT revision FROM services WHERE id=?',(id,)).fetchone()[0]
            if revision is not None and revision!=current: raise Problem(409,'Observation revision conflict')
            db.execute('INSERT OR REPLACE INTO health VALUES(?,?,?,?,?)',(id,d['status'],d['evidence'],now,now+ttl))
        return self.get(id)
    def export(self):
        # One read transaction: records and histories describe the same snapshot.
        with self.db() as db:
            db.execute('BEGIN')
            services=[{'id':r['id'],'revision':r['revision'],'updated_at':r['updated'],'spec':json.loads(r['document'])} for r in db.execute('SELECT * FROM services ORDER BY id')]
            history={r['id']:[] for r in services}
            for row in db.execute('SELECT id,revision,actor,updated,document FROM history ORDER BY seq'):
                h=dict(row); history[h.pop('id')].append(h)
        return {'exported_at':time.time(),'services':services,'history':history}
    def history(self,id):
        self.get(id)
        with self.db() as db: return [dict(r) for r in db.execute('SELECT revision,actor,updated,document FROM history WHERE id=? ORDER BY revision',(id,))]

class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self,*args,**kwargs): return None

def probe(catalog,record):
    p=record['spec'].get('probe')
    if not p or p.get('vantage','registry')!='registry' or record['spec']['lifecycle']!='active': return
    try:
        # No proxies or credentials. Only explicitly registered, writer-controlled URLs.
        from urllib.request import ProxyHandler
        if 'tcp_host' in p:
            from tcp_probe import check
            ok,evidence=check(p)
        else:
            headers={}
            if 'bearer_ref' in p:
                credential=catalog.probe_credentials[p['bearer_ref']]
                if credential['url']!=p['url']: raise ValueError('Credential target mismatch')
                headers['Authorization']='Bearer '+credential['token']
            with build_opener(ProxyHandler({}),NoRedirect()).open(Request(p['url'],headers=headers,method='GET'),timeout=3) as response:
                code=response.status
                body=json.loads(response.read(1048576)) if 'json_not_in' in p else None
            ok=code==p.get('expected_status',200)
            evidence='HTTP '+str(code)
            if 'json_not_in' in p:
                ready=isinstance(body,dict) and all(isinstance(body.get(k),str) and body[k] not in excluded for k,excluded in p['json_not_in'].items())
                ok=ok and ready
                evidence+='; application state '+('available' if ready else 'unavailable or missing')
    except Exception as e:
        ok=False; evidence='Probe failed: '+type(e).__name__
    try: catalog.observe(record['id'],{'status':'healthy' if ok else 'unhealthy','evidence':evidence,'ttl_seconds':1800},record['revision'])
    except Problem as e:
        if e.status!=409: raise

def monitor(catalog,stop):
    while not stop.is_set():
        try:
            with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
                list(pool.map(lambda r:probe(catalog,r),catalog.listing({})))
        except Exception: LOG.exception('Health reconciliation failed')
        stop.wait(600)

class Handler(BaseHTTPRequestHandler):
    server_version='LabRegistry/1'
    def setup(self): super().setup(); self.connection.settimeout(10)
    def log_message(self,*args): pass # No request headers or service data in access logs.
    def send(self,status,data):
        raw=json.dumps(data).encode(); self.send_response(status)
        self.send_header('Content-Type','application/json'); self.send_header('Content-Length',str(len(raw)))
        self.send_header('Cache-Control','no-store'); self.end_headers(); self.wfile.write(raw)
    def body(self):
        try: length=int(self.headers.get('Content-Length','0'))
        except ValueError: raise Problem(400,'Invalid content length')
        if not 0<length<=65536: raise Problem(413,'Body must be 1..65536 bytes')
        try: return json.loads(self.rfile.read(length))
        except (ValueError,UnicodeError): raise Problem(400,'Invalid JSON')
    def handle_api(self):
        try:
            u=urlsplit(self.path); parts=u.path.strip('/').split('/'); cat=self.server.catalog
            if self.command=='GET' and u.path=='/healthz': return self.send(200,{'ok':True})
            auth=self.headers.get('Authorization','')
            if self.command=='POST' and len(parts)==4 and parts[:2]==['v1','machines'] and parts[3]=='presence' and ID.fullmatch(parts[2]):
                enrollment=presence.authenticate(self.server.enrollments,parts[2],auth)
                if enrollment is None:raise Problem(401,'Machine credential required')
                return self.send(200,cat.announce(parts[2],self.body(),enrollment))
            writer=hmac.compare_digest(auth,'Bearer '+self.server.writer)
            reader=hmac.compare_digest(auth,'Bearer '+self.server.reader)
            if not (writer or reader): raise Problem(401,'Authentication required')
            if self.command!='GET' and not writer: raise Problem(403,'Write credential required')
            if self.command=='GET' and u.path=='/v1/export': return self.send(200,cat.export())
            if self.command=='GET' and u.path=='/v1/services': return self.send(200,{'services':cat.listing(parse_qs(u.query))})
            if len(parts)<3 or parts[:2]!=['v1','services'] or not ID.fullmatch(parts[2]): raise Problem(404,'Unknown route')
            id=parts[2]
            if self.command=='GET' and len(parts)==3: return self.send(200,cat.get(id))
            if self.command=='GET' and parts[3:]==['history']: return self.send(200,{'history':cat.history(id)})
            if self.command=='PUT' and len(parts)==3:
                header=self.headers.get('If-Match')
                if header is None: raise Problem(428,'If-Match revision required; use 0 to create')
                try: revision=int(header.strip('"'))
                except ValueError: raise Problem(400,'Invalid revision')
                return self.send(201 if revision==0 else 200,cat.put(id,self.body(),revision))
            if self.command=='POST' and parts[3:]==['observe']:
                header=self.headers.get('If-Match')
                try: revision=int(header) if header is not None else None
                except ValueError: raise Problem(400,'Invalid revision')
                return self.send(200,cat.observe(id,self.body(),revision))
            raise Problem(404,'Unknown route')
        except Problem as e: self.send(e.status,{'error':e.message})
        except Exception:
            LOG.exception('Request failed'); self.send(500,{'error':'Internal error'})
    do_GET=handle_api; do_PUT=handle_api; do_POST=handle_api

def make_server(address,database,reader,writer):
    s=ThreadingHTTPServer(address,Handler); s.catalog=Catalog(database); s.reader=reader; s.writer=writer; s.enrollments={}; return s

def main():
    from pathlib import Path
    p=argparse.ArgumentParser(); p.add_argument('--config',required=True); args=p.parse_args()
    c=json.loads(Path(args.config).read_text())
    logging.basicConfig(level=logging.INFO)
    s=make_server((c.get('bind','0.0.0.0'),c.get('port',8443)),c['database'],c['reader_token'],c['writer_token'])
    s.enrollments=c.get('machine_enrollments',{})
    if s.enrollments:s.catalog.presence_verifier=presence.verifier(c['presence_tls'])
    s.catalog.probe_credentials=c.get('probe_credentials',{})
    context=ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER); context.minimum_version=ssl.TLSVersion.TLSv1_2
    context.load_cert_chain(c['cert'],c['key']); s.socket=context.wrap_socket(s.socket,server_side=True)
    stop=threading.Event(); threading.Thread(target=monitor,args=(s.catalog,stop),daemon=True).start()
    LOG.info('Registry listening on port %s',s.server_port)
    try: s.serve_forever()
    finally: stop.set(); s.server_close()
if __name__=='__main__': main()
