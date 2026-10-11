import unittest
from unittest.mock import patch
from resolve import ssh_target
class ResolveTest(unittest.TestCase):
    def test_requires_current_presence_and_returns_stable_key_alias(self):
        record={'spec':{'lifecycle':'active','endpoints':{'ssh':'ssh://192.0.2.11:2222'},'resources':{'agent_presence':{'status':'current'}}}}
        with patch('resolve.request',return_value=record):
            self.assertEqual(ssh_target({},'machine-test','backup'),{'destination':'backup@192.0.2.11','port':2222,'host_key_alias':'machine-test'})
            record['spec']['resources']['agent_presence']['status']='stale'
            with self.assertRaises(ValueError):ssh_target({},'machine-test','backup')
