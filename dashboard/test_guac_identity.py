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

class ReviewAccess(unittest.TestCase):
    def test_disposable_terminal_only(self):
        from unittest.mock import patch
        from access import scoped_records
        target = {'id': 'stage', 'spec': {'lifecycle': 'active', 'capabilities': ['machine'],
                  'host': 'parent', 'resources': {'staging_review': {'access': {
                      'terminal_id': 'review', 'management_access': [
                          {'id': 'os', 'url': 'ssh://192.0.2.7', 'username': 'review'}]}}}}}
        parent = {'id': 'parent', 'spec': {'lifecycle': 'active', 'capabilities': ['machine'],
                  'host': 'parent', 'resources': {'management_access': [
                      {'id': 'os', 'url': 'ssh://192.0.2.8', 'username': 'admin'}],
                      'console_login': {'password_available': True}}}}
        records = [target, parent]
        with tempfile.TemporaryDirectory() as directory, patch.dict(
                'os.environ', {'LAB_DASHBOARD_ACCESS_TARGET': 'stage'}):
            private = pathlib.Path(directory)
            (private/'terminal_key').write_text('test-key')
            (private/'guacamole_known_hosts').write_text('192.0.2.7 ssh-ed25519 key\n192.0.2.8 ssh-ed25519 key\n')
            scoped = scoped_records(records)
            self.assertFalse(scoped[1]['spec']['resources']['management_access'])
            self.assertIsNone(scoped[1]['spec']['resources']['console_login'])
            self.assertEqual(list(connections(private, records)), ['review'])
            self.assertTrue(parent['spec']['resources']['console_login']['password_available'])
            with patch.dict('os.environ', {'LAB_DASHBOARD_ACCESS_TARGET': 'missing'}):
                self.assertEqual(connections(private, records), {})
