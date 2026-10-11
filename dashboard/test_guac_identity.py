import pathlib,tempfile,unittest
from guac import connections
class GuacIdentity(unittest.TestCase):
    def test_stable_alias_is_rebound_to_current_ip_with_same_key(self):
        with tempfile.TemporaryDirectory() as d:
            p=pathlib.Path(d);(p/'terminal_key').write_text('test-only-key')
            (p/'guacamole_known_hosts').write_text('machine-test ssh-ed25519 AAAAtest\n')
            r={'spec':{'lifecycle':'active','capabilities':['machine'],'host':'test-node','resources':{'management_access':[{'id':'os','url':'ssh://192.0.2.11:2222','username':'tester','host_key_alias':'machine-test'}]}}}
            params=connections(p,[r])['test-node']['parameters']
            self.assertEqual(params['hostname'],'192.0.2.11')
            self.assertEqual(params['host-key'],'[192.0.2.11]:2222 ssh-ed25519 AAAAtest')
            r['spec']['resources']['management_access'][0]['host_key_alias']='unknown'
            self.assertEqual(connections(p,[r]),{})
