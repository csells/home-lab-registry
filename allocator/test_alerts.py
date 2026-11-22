import json
import subprocess
import unittest
from unittest.mock import patch

from alerts import deliver


class ExportAlertWorkerTest(unittest.TestCase):
    config = {
        'endpoint': 'https://controller.example',
        'ca_file': 'ca.pem',
        'alert_token_file': 'alert.token',
        'delivery_command': ['deliver-alert'],
    }
    event = {'id': 'a' * 32, 'owner': 'test-task', 'lease_id': 'b' * 32, 'failed_at': 42}

    def test_failed_delivery_leaves_alert_pending(self):
        with (patch('alerts.request', return_value={'alert': self.event}) as request,
              patch('alerts.subprocess.run', side_effect=subprocess.CalledProcessError(75, 'deliver-alert'))):
            with self.assertRaises(subprocess.CalledProcessError):
                deliver(self.config)
        self.assertEqual(request.call_count, 1)
        self.assertIsNone(request.call_args.args[-1])

    def test_success_sends_only_safe_fields_and_acknowledges_same_event(self):
        response = {'alert': {**self.event, 'export_error': 'raw private diagnostic'}}
        with (patch('alerts.request', side_effect=[response, {'delivered': True}]) as request,
              patch('alerts.subprocess.run') as run):
            self.assertTrue(deliver(self.config))
        payload = json.loads(run.call_args.kwargs['input'])
        self.assertEqual(payload, {**self.event, 'kind': 'lease-export-failed'})
        self.assertEqual(request.call_args.args[-1], {'alert_id': self.event['id']})
