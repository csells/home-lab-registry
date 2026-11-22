import json, tempfile, threading, unittest, urllib.request, urllib.error
from registry import make_server

class RegistryTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.server = make_server(('127.0.0.1', 0), self.tmp.name+'/catalog.db', 'test-reader', 'test-writer')
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True); self.thread.start()
        self.base = 'http://127.0.0.1:'+str(self.server.server_port)
    def tearDown(self):
        self.server.shutdown(); self.server.server_close(); self.tmp.cleanup()
    def call(self, method, path, body=None, token='test-writer', revision=None):
        headers={'Authorization':'Bearer '+token, 'Content-Type':'application/json'}
        if revision is not None: headers['If-Match']=str(revision)
        req=urllib.request.Request(self.base+path, data=None if body is None else json.dumps(body).encode(), headers=headers, method=method)
        try: response = urllib.request.urlopen(req)
        except urllib.error.HTTPError as error: response = error
        with response:
            return response.status, json.load(response)
    def record(self):
        return dict(name='Voice', host='desktop-node', owner='example-owner', capabilities=['tts','stt'], endpoints={'api':'https://example.test'}, lifecycle='active', runbook='https://example.test/runbook', notes='test', dependencies=[])
    def test_access_validation_and_conflicts(self):
        self.assertEqual(self.call('GET','/v1/services',token='wrong')[0],401)
        self.assertEqual(self.call('PUT','/v1/services/voice',self.record(),token='test-reader',revision=0)[0],403)
        self.assertEqual(self.call('PUT','/v1/services/voice',{})[0],428)
        self.assertEqual(self.call('PUT','/v1/services/voice',{},revision=0)[0],400)
        code,r=self.call('PUT','/v1/services/voice',self.record(),revision=0)
        self.assertEqual(code,201); self.assertEqual(r['revision'],1)
        self.assertEqual(self.call('PUT','/v1/services/voice',self.record(),revision=0)[0],409)
        self.assertEqual(self.call('GET','/v1/services?capability=tts',token='test-reader')[1]['services'][0]['id'],'voice')
        self.assertEqual(self.call('GET','/v1/services?capability=gpu')[1]['services'],[])
    def test_update_retire_audit_and_persistence(self):
        self.call('PUT','/v1/services/voice',self.record(),revision=0)
        r=self.record(); r['name']='Updated'
        self.assertEqual(self.call('PUT','/v1/services/voice',r,revision=1)[1]['revision'],2)
        r['lifecycle']='retired'
        self.assertEqual(self.call('PUT','/v1/services/voice',r,revision=2)[0],200)
        self.assertEqual(self.call('GET','/v1/services')[1]['services'],[])
        self.assertEqual(len(self.call('GET','/v1/services?include_retired=true')[1]['services']),1)
        self.assertEqual(len(self.call('GET','/v1/services/voice/history')[1]['history']),3)
        other=make_server(('127.0.0.1',0),self.tmp.name+'/catalog.db','test-reader','test-writer')
        self.assertEqual(other.catalog.get('voice')['revision'],3); other.server_close()
    def test_runtime_report_is_scoped_to_the_registered_deployment(self):
        binding = {'kind': 'native', 'provider': 'systemd', 'instance': 'host:test.service'}
        doc = self.record()
        doc['resources'] = {'runtime_binding': binding}
        token = 'r' * 40
        self.server.runtime_publishers = {'voice': {'token': token, 'binding': binding}}
        self.assertEqual(self.call('PUT', '/v1/services/voice', doc, revision=0)[0], 201)
        body = {'state': 'present', 'ttl_seconds': 900}
        route = '/v1/services/voice/runtime'
        self.assertEqual(self.call('POST', route, body, token='test-writer')[0], 401)
        self.assertEqual(self.call('GET', '/v1/services', token=token)[0], 401)
        self.assertEqual(self.call('POST', '/v1/services/other/runtime', body, token=token)[0], 401)
        status, value = self.call('POST', route, body, token=token)
        self.assertEqual(status, 200)
        self.assertEqual(self.call('POST', route, body, token=token)[1], value)
        self.assertEqual(self.call('GET', '/v1/services/voice')[1]['runtime_observation'], value)
        doc['name'] = 'Updated service'
        self.assertEqual(self.call('PUT', '/v1/services/voice', doc, revision=1)[0], 200)
        self.assertEqual(self.call('GET', '/v1/services/voice')[1]['runtime_observation'], value)
        doc['resources']['runtime_binding'] = {**binding, 'instance': 'host:replacement.service'}
        self.assertEqual(self.call('PUT', '/v1/services/voice', doc, revision=2)[0], 200)
        self.assertEqual(self.call('POST', route, body, token=token)[0], 409)
        self.assertNotIn('runtime_observation', self.call('GET', '/v1/services/voice')[1])

    def test_change_stream_tracks_only_committed_metadata(self):
        path = '/v1/catalog/changes'
        epoch = self.server.catalog.change_epoch
        self.assertEqual(self.call('GET', path, token='invalid')[0], 401)
        def open_feed(cursor=None):
            headers = {'Authorization': 'Bearer test-reader'}
            if cursor is not None:
                headers['Last-Event-ID'] = cursor
            return urllib.request.urlopen(urllib.request.Request(self.base + path, headers=headers), timeout=3)

        with open_feed() as stream:
            self.assertEqual(stream.headers.get_content_type(), 'application/x-ndjson')
            self.assertEqual(json.loads(stream.readline()), {'epoch': epoch, 'version': 0})
            self.assertEqual(self.call('PUT', '/v1/services/voice', self.record(), revision=0)[0], 201)
            self.assertEqual(json.loads(stream.readline()), {'epoch': epoch, 'version': 1})
        with open_feed(f'{epoch}:1') as stream:
            self.assertEqual(json.loads(stream.readline()), {'heartbeat': True})
            observation = {'status': 'healthy', 'evidence': 'test'}
            self.assertEqual(self.call('POST', '/v1/services/voice/observe', observation)[0], 200)
            self.assertEqual(self.call('PUT', '/v1/services/voice', self.record(), revision=0)[0], 409)
        doc = self.record()
        doc['lifecycle'] = 'retired'
        self.assertEqual(self.call('PUT', '/v1/services/voice', doc, revision=1)[0], 200)
        self.assertEqual(self.call('GET', '/v1/services')[1]['services'], [])
        for cursor in (f'{epoch}:1', f'{"0" * 32}:2'):
            with open_feed(cursor) as stream:
                self.assertEqual(json.loads(stream.readline()), {'epoch': epoch, 'version': 2})
        with self.assertRaises(urllib.error.HTTPError) as error:
            open_feed('invalid')
        self.assertEqual(error.exception.code, 400)
        error.exception.close()

    def test_health_is_separate_and_expires(self):
        self.call('PUT','/v1/services/voice',self.record(),revision=0)
        self.assertEqual(self.call('GET','/v1/services/voice')[1]['health']['status'],'unknown')
        self.assertEqual(self.call('POST','/v1/services/voice/observe',{'status':'healthy','evidence':'real audio passed','ttl_seconds':60})[0],200)
        r=self.call('GET','/v1/services/voice')[1]
        self.assertEqual(r['health']['status'],'healthy'); self.assertEqual(r['revision'],1)
        self.assertEqual(self.call('POST','/v1/services/voice/observe',{'status':'healthy','evidence':'x','ttl_seconds':0})[0],400)
    def test_export_and_stale_health(self):
        self.call('PUT','/v1/services/voice',self.record(),revision=0)
        self.call('POST','/v1/services/voice/observe',{'status':'healthy','evidence':'check','ttl_seconds':1})
        with self.server.catalog.db() as db: db.execute('UPDATE health SET expires=0')
        self.assertEqual(self.call('GET','/v1/services/voice')[1]['health']['status'],'stale')
        self.assertEqual(self.call('POST','/v1/services/voice/observe',{'status':'healthy','evidence':'old check','ttl_seconds':60},revision=0)[0],409)
        e=self.call('GET','/v1/export')[1]
        self.assertEqual(e['services'][0]['revision'],len(e['history']['voice']))

    def test_secrets_and_unsafe_probe(self):
        r=self.record(); r['password']='oops'
        self.assertEqual(self.call('PUT','/v1/services/x',r,revision=0)[0],400)
        r=self.record(); r['probe']={'url':'file:///etc/passwd'}
        self.assertEqual(self.call('PUT','/v1/services/x',r,revision=0)[0],400)
        r=self.record(); r['endpoints']={'api':'https://user:secret@example.com'}
        self.assertEqual(self.call('PUT','/v1/services/x',r,revision=0)[0],400)

if __name__=='__main__': unittest.main()
