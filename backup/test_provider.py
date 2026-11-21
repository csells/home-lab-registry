import unittest

from provider import verified_address
from proxmox_observe import guest_request


class ProviderIdentity(unittest.TestCase):
    def setUp(self):
        self.binding = {'vmid': 123, 'guest_kind': 'qemu', 'uuid': 'stable-uuid',
                        'mac': 'bc:24:11:00:00:01', 'allowed_networks': ['192.0.2.0/24']}
        self.observed = {'vmid': 123, 'kind': 'qemu', 'uuid': 'stable-uuid',
                         'state': 'running', 'observed_at': 1000,
                         'interfaces': [{'hardware-address': 'BC:24:11:00:00:01',
                                         'ip-addresses': [{'ip-address': '192.0.2.42'}]}]}

    def test_qemu_uuid_and_mac_select_live_address(self):
        self.assertEqual(verified_address(self.binding, self.observed, 1001), '192.0.2.42')

    def test_lxc_uses_volume_identity_without_inventing_uuid(self):
        self.binding.update(guest_kind='lxc', volume_id='pool:vm-123-disk-0')
        del self.binding['uuid']
        self.observed.update(kind='lxc', volume_id='pool:vm-123-disk-0')
        del self.observed['uuid']
        self.assertEqual(verified_address(self.binding, self.observed, 1001), '192.0.2.42')
        self.observed['volume_id'] = 'pool:replacement'
        with self.assertRaises(ValueError):
            verified_address(self.binding, self.observed, 1001)

    def test_wrong_identity_mac_kind_state_and_time_fail_closed(self):
        for field, value in [('vmid', 124), ('uuid', 'replaced'), ('kind', 'lxc'),
                             ('state', 'stopped'), ('observed_at', 900),
                             ('observed_at', 1010), ('observed_at', True)]:
            with self.subTest(field=field, value=value):
                other = dict(self.observed, **{field: value})
                with self.assertRaises(ValueError):
                    verified_address(self.binding, other, 1001)
        self.binding['mac'] = 'bc:24:11:00:00:02'
        with self.assertRaises(ValueError):
            verified_address(self.binding, self.observed, 1001)

    def test_multiple_allowed_addresses_and_unallowed_address_fail_closed(self):
        self.observed['interfaces'][0]['ip-addresses'].append({'ip-address': '192.0.2.43'})
        with self.assertRaises(ValueError):
            verified_address(self.binding, self.observed, 1001)

        self.binding['allowed_networks'] = ['198.51.100.0/24']
        self.observed['interfaces'][0]['ip-addresses'] = [{'ip-address': '192.0.2.42'}]
        with self.assertRaises(ValueError):
            verified_address(self.binding, self.observed, 1001)

    def test_forced_observer_rejects_unlisted_guests_and_shell_commands(self):
        self.assertEqual(guest_request('discover 123', {'123': 'qemu'}), ('123', 'qemu'))
        for command in ['discover 124', 'discover 123; id', 'export', 'discover ../123',
                        'discover 00123', 'discover 123 extra']:
            with self.subTest(command=command), self.assertRaises(ValueError):
                guest_request(command, {'123': 'qemu'})


if __name__ == '__main__':
    unittest.main()
