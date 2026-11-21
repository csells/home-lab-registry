import sqlite3
import unittest

from review import lease_page


class LeaseReviewTest(unittest.TestCase):
    def test_current_lease_older_than_first_history_page(self):
        db = sqlite3.connect(':memory:')
        self.addCleanup(db.close)
        db.row_factory = sqlite3.Row
        db.execute("""CREATE TABLE lease (id TEXT, task TEXT, owner TEXT, phase TEXT,
                   deadline REAL, released_at REAL, hold_mode TEXT, hold_reason TEXT,
                   reviewer TEXT, reviewer_hold TEXT, reviewer_reason TEXT,
                   export_error TEXT, checkpoint TEXT)""")
        db.execute('CREATE TABLE incremental_checkpoint (lease_id TEXT, receipt TEXT)')
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
