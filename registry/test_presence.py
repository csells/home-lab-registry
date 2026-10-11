import hashlib,json,time,unittest
import test_registry as base

class PresenceTest(unittest.TestCase):
    tearDown=base.RegistryTest.tearDown
    call=base.RegistryTest.call
    record=base.RegistryTest.record
    def setUp(self):
        base.RegistryTest.setUp(self)
        self.token='machine-test-token-with-sufficient-entropy'
        self.server.enrollments={'machine-test':{'token_sha256':hashlib.sha256(self.token.encode()).hexdigest(),'allowed_networks':['192.0.2.0/24'],'tls_name':'machine-test'}}
        record=self.record();record.update(capabilities=['machine'],endpoints={'ssh':'ssh://192.0.2.1','kvm':'https://192.0.2.2'})
        record['resources']={'registration':{'endpoint_keys':['ssh'],'metrics':True,'probe':True},'metrics_stream':{'enabled':True,'url':'https://192.0.2.1:9911/stream','server_name':'machine-test'}}
        record['probe']={'tcp_host':'192.0.2.1','tcp_port':22}
        self.call('PUT','/v1/services/machine-test',record,revision=0)
    def announce(self,address='192.0.2.10',token=None,machine='machine-test'):
        return self.call('POST','/v1/machines/'+machine+'/presence',{'address':address,'interface':'eth0'},token=token or self.token)
    def test_scoped_identity_network_and_retirement(self):
        self.assertEqual(self.announce(token='wrong')[0],401)
        self.assertEqual(self.announce(machine='machine-other')[0],401)
        self.assertEqual(self.announce('127.0.0.1')[0],400)
        self.assertEqual(self.announce('198.51.100.5')[0],400)
        self.assertEqual(self.announce()[0],200)
        self.assertEqual(self.call('GET','/v1/services',token=self.token)[0],401)
        record=self.server.catalog.get('machine-test');record['spec']['lifecycle']='retired';self.server.catalog.put('machine-test',record['spec'],record['revision'])
        self.assertEqual(self.announce()[0],403)
    def test_changes_discovery_without_revision_churn_or_kvm_rewrite(self):
        self.assertEqual(self.announce()[0],200)
        self.assertEqual(self.announce('192.0.2.11')[0],200)
        r=self.server.catalog.get('machine-test');s=r['spec']
        self.assertEqual(r['revision'],1)
        self.assertEqual(s['endpoints']['ssh'],'ssh://192.0.2.11')
        self.assertEqual(s['endpoints']['kvm'],'https://192.0.2.2')
        self.assertEqual(s['probe']['tcp_host'],'192.0.2.11')
        self.assertEqual(s['resources']['metrics_stream']['server_name'],'machine-test')
        self.assertEqual(s['resources']['agent_presence']['status'],'current')
        with self.server.catalog.db() as db:db.execute('UPDATE presence SET expires=0')
        self.assertEqual(self.server.catalog.get('machine-test')['spec']['resources']['agent_presence']['status'],'stale')
        exported=self.server.catalog.export()['services'][0]['spec']
        self.assertNotIn('agent_presence',exported['resources'])
    def test_new_address_requires_verified_collector_identity(self):
        def reject(*args):raise ValueError('certificate mismatch')
        self.server.catalog.presence_verifier=reject
        self.assertEqual(self.announce()[0],409)
        self.assertNotIn('agent_presence',self.server.catalog.get('machine-test')['spec']['resources'])

    def test_service_bindings_follow_guest_without_rewriting_public_url(self):
        service=self.record();service['endpoints']={'web':'https://public.example','health':'http://192.0.2.1:8000/health'}
        service['probe']={'url':'http://192.0.2.1:8000/health'}
        service['resources']={'address_bindings':[{'machine_id':'machine-test','endpoint_keys':['health'],'probe':True}]}
        self.assertEqual(self.call('PUT','/v1/services/application',service,revision=0)[0],201)
        self.announce('192.0.2.11')
        r=self.server.catalog.get('application')['spec']
        self.assertEqual(r['endpoints']['web'],'https://public.example')
        self.assertEqual(r['endpoints']['health'],'http://192.0.2.11:8000/health')
        self.assertEqual(r['probe']['url'],'http://192.0.2.11:8000/health')
        service['resources']['address_bindings']=[{'machine_id':42}]
        self.assertEqual(self.call('PUT','/v1/services/application',service,revision=1)[0],400)
