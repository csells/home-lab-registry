import unittest
from unittest.mock import patch

from client import request


class RegistryTransportTests(unittest.TestCase):
    def test_dashboard_refuses_cleartext_registry_before_connecting(self):
        config = {'url': 'http://registry.invalid', 'reader_token': 'test-reader', 'ca_file': '/missing'}
        with patch('urllib.request.urlopen', side_effect=AssertionError('Unexpected network request')) as connect:
            with self.assertRaisesRegex(RuntimeError, 'HTTPS'):
                request(config, 'GET', '/v1/services')
        connect.assert_not_called()
