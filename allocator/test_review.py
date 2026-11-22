import sqlite3
import unittest
from unittest.mock import patch

from review import lease_inventory, lease_page


class LeaseReviewTest(unittest.TestCase):
    def test_issues_project_only_current_overdue_or_failed_leases(self):
        with sqlite3.connect(':memory:') as db:
            db.row_factory = sqlite3.Row
            db.execute("""CREATE TABLE lease (id, task, phase, deadline, export_error, hold_mode, reviewer_hold)""")
            db.executemany('INSERT INTO lease VALUES (?,?,?,?,?,?,?)', [
                ('1', 'Overdue', 'ready', 90, None, None, None),
                ('2', 'Held', 'ready', 90, None, 'running', None),
                ('3', 'Export blocked', 'awaiting-export', 80, 'private diagnostics', None, None),
                ('4', 'Next', 'ready', 120, None, None, None),
                ('5', 'History', 'released', 10, 'private diagnostics', None, None),
            ])
            with patch('review.time.time', return_value=100):
                result = lease_page(db, None, 'issues')
            self.assertEqual(result['issues'], [
                {'task':'Overdue','code':'overdue'}, {'task':'Export blocked','code':'export-failed'}])
            self.assertEqual(result['next_deadline'], 120)
            self.assertEqual(result['counts'], {'current':4, 'released':1})

    def test_inventory_projects_only_active_dynamic_intent(self):
        class Scope:
            node = 'provider'
            guest_id = '102'

            def name(self, lease):
                return 'lease-' + lease

        with sqlite3.connect(':memory:') as db:
            db.row_factory = sqlite3.Row
            db.execute('CREATE TABLE lease(id, phase, slot_id)')
            db.executemany('INSERT INTO lease VALUES (?, ?, ?)', [
                ('first', 'cloning', 'dynamic'), ('second', 'released', 'dynamic'),
                ('third', 'ready', 'fixed')])
            self.assertEqual(lease_inventory(db, {'dynamic': Scope()}), {'guests': [
                {'node': 'provider', 'id': '102', 'name': 'lease-first', 'type': 'qemu', 'lease': 'first'},
            ]})

    def test_current_lease_older_than_first_history_page(self):
        db = sqlite3.connect(':memory:')
        self.addCleanup(db.close)
        db.row_factory = sqlite3.Row
        db.execute("""CREATE TABLE lease (id TEXT, task TEXT, owner TEXT, phase TEXT,
                   deadline REAL, released_at REAL, hold_mode TEXT, hold_reason TEXT,
                   reviewer TEXT, reviewer_hold TEXT, reviewer_reason TEXT,
                   export_error TEXT, checkpoint TEXT)""")
        db.execute('CREATE TABLE incremental_checkpoint (id TEXT, lease_id TEXT, receipt TEXT)')
        db.execute("CREATE TABLE restore_hold (replacement TEXT, source TEXT, checkpoint_id TEXT)")
        db.execute("INSERT INTO lease (id,phase) VALUES ('held','awaiting-export')")
        db.executemany("INSERT INTO lease (id,phase) VALUES (?,'released')",
                       [(str(i),) for i in range(30)])
        current = lease_page(db, None, 'current')
        summary = lease_page(db, None, 'counts')
        self.assertNotIn('leases', summary)
        self.assertEqual(summary['counts'], {'current': 1, 'released': 30})
        self.assertEqual(current['counts'], summary['counts'])
        self.assertEqual([row['id'] for row in current['leases']], ['held'])
        self.assertIsNone(current['next_before'])
        first = lease_page(db, None, 'released')
        second = lease_page(db, str(first['next_before']), 'released')
        self.assertEqual([row['id'] for row in first['leases'] + second['leases']],
                         [str(i) for i in reversed(range(30))])
        self.assertEqual(second['counts'], summary['counts'])
        self.assertIsNone(second['next_before'])
