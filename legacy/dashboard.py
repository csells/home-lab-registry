#!/usr/bin/env python3
"""Loopback-only read-only dashboard behind an authenticated reverse proxy."""
import argparse, json, logging, pathlib, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlsplit
from client import request

def public_record(record):
    spec=record['spec']
    backup=spec.get('config_backup',{'status':'unknown','locations':[]})
    backup={k:backup[k] for k in ('status','locations','notes') if k in backup}
    backup['locations']=[{k:loc[k] for k in ('host','path')} for loc in backup.get('locations',[])]
    return dict(config_backup=backup,id=record['id'],name=spec['name'],host=spec['host'],capabilities=spec['capabilities'],endpoints=spec['endpoints'],lifecycle=spec['lifecycle'],health={k:v for k,v in record['health'].items() if k in ('status','evidence','checked','expires')})

class Handler(BaseHTTPRequestHandler):
    def log_message(self,*args): pass
    def setup(self): super().setup();self.connection.settimeout(20)
    def respond(self,code,data,content_type):
        self.send_response(code);self.send_header('Content-Type',content_type);self.send_header('Content-Length',str(len(data)))
        self.send_header('Cache-Control','no-store');self.send_header('X-Content-Type-Options','nosniff');self.send_header('Referrer-Policy','no-referrer')
        self.send_header('Content-Security-Policy',"default-src 'self'; script-src 'self'; style-src 'self'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'")
        self.end_headers();self.wfile.write(data)
    def do_GET(self):
        path=urlsplit(self.path).path
        if path=='/api/services':
            try:
                records=request(self.server.config,'GET','/v1/services')['services']
                data={'fetched_at':time.time(),'services':[public_record(r) for r in records]}
                return self.respond(200,json.dumps(data).encode(),'application/json')
            except Exception:
                logging.exception('Registry fetch failed')
                return self.respond(502,b'{"error":"Registry unavailable. Last displayed readings may be out of date."}','application/json')
        files={'/':('dashboard.html','text/html; charset=utf-8'),'/dashboard.js':('dashboard.js','text/javascript; charset=utf-8'),'/dashboard.css':('dashboard.css','text/css; charset=utf-8')}
        if path not in files:return self.respond(404,b'Not found','text/plain')
        filename,kind=files[path];self.respond(200,(pathlib.Path(__file__).parent/filename).read_bytes(),kind)

def main():
    p=argparse.ArgumentParser();p.add_argument('--port',type=int,default=8877);p.add_argument('--config',default='~/.config/home-lab/registry.json');a=p.parse_args()
    cfg=json.loads(pathlib.Path(a.config).expanduser().read_text())
    # Keep only the read credential in memory; never forward the upstream API or expose mutations.
    cfg={k:cfg[k] for k in ('url','ca_file','reader_token')}
    s=ThreadingHTTPServer(('127.0.0.1',a.port),Handler);s.config=cfg;s.serve_forever()
if __name__=='__main__':main()
