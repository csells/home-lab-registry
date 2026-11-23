"""Verify the provider-only controller boundary without dispatching anything."""
import unittest
from unittest.mock import patch

from checkpoints import Rejected
from provider_adapter import Adapter


class ProviderAdapterTest(unittest.TestCase):
    def test_unknown_actions_cannot_reach_provider_credentials_or_network(self):
        with patch('provider_adapter.Slot.configure') as configure:
            with self.assertRaises(Rejected) as problem:
                Adapter({}).invoke({'action':'shell','command':'unrelated'})
            self.assertEqual(problem.exception.code,'provider_operation_unknown')
            configure.assert_not_called()

    def test_provider_mutation_requires_a_matching_persisted_guest_generation(self):
        with patch('provider_adapter.Slot.configure') as configure:
            scope=configure.return_value
            scope.apple = scope.dynamic = None
            scope.deletes_on_release = False
            scope.provider.generation.return_value='current'
            with self.assertRaises(Rejected) as problem:
                Adapter({}).invoke({'action':'dispatch','slot_id':'primary',
                    'lease':'a'*32,'phase':'starting','generation':'stale'})
            self.assertEqual(problem.exception.code,'slot_reconciliation_required')
            scope.provider.start.assert_not_called()
