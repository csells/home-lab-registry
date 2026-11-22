import unittest

from checkpoints import Rejected
from maintenance import plan
from pve_dynamic import DynamicScope


class MaintenanceTest(unittest.TestCase):
    recipe = {
        'worker': 'worker', 'storage': 'vm-images', 'network': 'dhcp',
        'image_sha256': 'a' * 64, 'bootstrap_revision': 'v1',
        'memory_mib': 2048, 'vcpus': 2, 'disk_mib': 8192,
    }

    def test_replacement_reports_owners_and_exact_overlap_shortfall(self):
        owners = [{'id': 'a' * 32, 'owner': 'thread-owner'}]
        result = plan(self.recipe, {**self.recipe, 'bootstrap_revision': 'v2'}, owners,
                      {'memory_mib': 1024, 'vcpus': 2, 'disk_mib': 10240})
        self.assertEqual(result['action'], 'replace')
        self.assertEqual(result['affected_leases'], owners)
        self.assertEqual(result['missing_capacity'], {'memory_mib': 1024})
        self.assertEqual(result['blocker'], 'overlap_capacity_unavailable')
        self.assertTrue(result['requires_verified_checkpoint'])

    def test_clone_network_identity_ignores_only_the_mac(self):
        config = {'net0': 'virtio=02:00:00:00:00:01,bridge=vmbr0,firewall=1',
                  'ipconfig0': 'ip=dhcp'}
        other = {**config, 'net0': 'virtio=02:00:00:00:00:02,firewall=1,bridge=vmbr0'}
        self.assertEqual(DynamicScope.network(config), DynamicScope.network(other))
        with self.assertRaisesRegex(Rejected, 'single DHCP'):
            DynamicScope.network({**other, 'ipconfig1': 'ip=192.0.2.3/24'})
        self.assertEqual(DynamicScope.config_digest(config),
                         DynamicScope.config_digest({**config, 'digest': 'provider-value'}))
        self.assertNotEqual(DynamicScope.config_digest(config), DynamicScope.config_digest(other))
