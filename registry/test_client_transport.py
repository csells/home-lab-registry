import importlib.util, json, os, ssl, subprocess, tempfile, threading, unittest
from pathlib import Path
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

spec=importlib.util.spec_from_file_location('client',Path(__file__).with_name('client.py'))
client=importlib.util.module_from_spec(spec);spec.loader.exec_module(client)
counts={}
class Handler(BaseHTTPRequestHandler):
    protocol_version='HTTP/1.1'
    def log_message(self,*a): pass
    def do_GET(self): self.reply()
    def do_PUT(self): self.reply()
    def reply(self):
        counts[self.path]=counts.get(self.path,0)+1
        if self.headers.get('Authorization')!='Bearer test-token':
            self.send_response(401);self.send_header('Content-Length','0');self.end_headers();return
        if self.path=='/redirect':
            self.send_response(302);self.send_header('Location','https://localhost:1/stolen');self.send_header('Content-Length','0');self.end_headers();return
        if self.path=='/deny':
            self.send_response(403);self.send_header('Content-Length','0');self.end_headers();return
        if self.path=='/write':
            n=int(self.headers['Content-Length']);d=json.loads(self.rfile.read(n));assert d=={'note':'quotes " slash \\ newline\nUnicode λ'};assert self.headers['If-Match']=='7'
        broken=self.path in ('/truncated','/write-truncated') or (self.path=='/retry' and counts[self.path]==1)
        body=b'{broken' if self.path=='/invalid' else json.dumps({'ok':True,'payload':'x'*250000}).encode()
        self.send_response(200);self.send_header('Content-Length',str(len(body)));self.end_headers()
        if broken:
            self.wfile.write(body[:30]);self.wfile.flush();self.close_connection=True
        else:self.wfile.write(body)

class Tests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp=tempfile.TemporaryDirectory();p=Path(cls.tmp.name)
        subprocess.run(['openssl','req','-x509','-newkey','rsa:2048','-nodes','-keyout',str(p/'key'),'-out',str(p/'cert'),'-days','1','-subj','/CN=localhost','-addext','subjectAltName=DNS:localhost'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,check=True)
        cls.server=ThreadingHTTPServer(('127.0.0.1',0),Handler)
        ctx=ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER);ctx.load_cert_chain(p/'cert',p/'key');cls.server.socket=ctx.wrap_socket(cls.server.socket,server_side=True)
        threading.Thread(target=cls.server.serve_forever,daemon=True).start()
        cls.c={'url':'https://localhost:'+str(cls.server.server_port),'ca_file':str(p/'cert'),'reader_token':'test-token','writer_token':'test-token'}
    @classmethod
    def tearDownClass(cls):cls.server.shutdown();cls.server.server_close();cls.tmp.cleanup()
    def test_complete_large_response(self):self.assertEqual(len(client.request(self.c,'GET','/ok')['payload']),250000)
    def test_read_retry(self):self.assertTrue(client.request(self.c,'GET','/retry')['ok']);self.assertEqual(counts['/retry'],2)
    def test_partial_never_accepted(self):
        with self.assertRaises(client.RegistryTransportError):client.request(self.c,'GET','/truncated')
        self.assertEqual(counts['/truncated'],3)
    def test_write_never_replayed(self):
        with self.assertRaises(client.RegistryTransportError):client.request(self.c,'PUT','/write-truncated',{'a':1},7)
        self.assertEqual(counts['/write-truncated'],1)
    def test_write_encoding_and_revision(self):self.assertTrue(client.request(self.c,'PUT','/write',{'note':'quotes " slash \\ newline\nUnicode λ'},7)['ok'])
    def test_redirect_rejected(self):
        with self.assertRaisesRegex(client.RegistryTransportError,'HTTP 302'):client.request(self.c,'GET','/redirect')
    def test_denial_not_retried(self):
        with self.assertRaisesRegex(client.RegistryTransportError,'HTTP 403'):client.request(self.c,'GET','/deny')
        self.assertEqual(counts['/deny'],1)
    def test_invalid_json_rejected(self):
        with self.assertRaisesRegex(client.RegistryTransportError,'valid JSON'):client.request(self.c,'GET','/invalid')
    def test_tls_not_disabled(self):
        c=dict(self.c,ca_file='/etc/ssl/certs/ca-certificates.crt')
        with self.assertRaises(client.RegistryTransportError):client.request(c,'GET','/ok')
    def test_cleartext_rejected(self):
        with self.assertRaises(client.RegistryTransportError):client.request(dict(self.c,url='http://localhost'),'GET','/')
if __name__=='__main__':unittest.main(verbosity=2)
