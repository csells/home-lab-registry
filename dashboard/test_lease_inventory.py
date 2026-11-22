import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from lease_review import LeaseInventory
from reconciliation import reconcile


class LeaseInventoryTests(unittest.TestCase):
    def test_exact_live_clone_join_on_change_and_manual_refresh(self):
        lease = 'a' * 32
        guest = {'name': 'lease-' + lease, 'id': '102', 'type': 'qemu', 'state': 'running'}
        wrong = {**guest, 'id': '103'}
        sample = {'entries': [guest, wrong], 'checked': 100, 'expires': 200}
        records = [{'id': 'machine-h', 'spec': {
            'host': 'h', 'owner': 'team', 'resources': {'kind': 'machine'}}}]
        reports = {'machine-h': sample}
        with tempfile.TemporaryDirectory() as directory:
            config = Path(directory) / 'review.json'
            config.write_text(json.dumps({'inventory_machines': {'node': 'machine-h'}}))
            join = LeaseInventory(config)
        projected = {'guests': [{**guest, 'node': 'node', 'lease': lease}]}
        with patch('lease_review.page', return_value=projected) as page:
            for second in range(100, 105):
                sample['checked'] = second
                claims, unavailable = join.reconcile(reports, now=second)
            self.assertEqual(page.call_count, 1)
            current = reconcile(records, reports, now=104, lease_claims=claims, lease_unverified=unavailable)
            observed, unrelated = current['hosts'][0]['guests']
            self.assertEqual(observed['lease'], lease)
            self.assertNotIn('lease', unrelated)
            self.assertEqual([item['guest_id'] for item in current['findings']], ['103'])
            page.side_effect = OSError('offline')
            claims, unavailable = join.reconcile(reports, now=106, force=True)
            uncertain = reconcile(records, reports, now=106, lease_claims=claims, lease_unverified=unavailable)
            self.assertEqual({item['code'] for item in uncertain['findings']}, {'lease-ownership-unverified'})
            self.assertFalse(claims)
            sample['entries'] = []
            self.assertEqual(join.reconcile(reports, now=107), ({}, frozenset()))
            self.assertEqual(page.call_count, 2)
