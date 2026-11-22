import tempfile
import time
import unittest
from pathlib import Path
from threading import Event, Thread
from unittest.mock import patch

from lease_review import LeaseIssueFeed


class LeaseIssueFeedTests(unittest.TestCase):
    def test_one_viewer_driven_feed_notifies_failure_and_recovery(self):
        counts = {'current': 1, 'released': 3}
        with tempfile.TemporaryDirectory() as directory:
            config = Path(directory) / 'review.json'
            config.touch()
            changed = Event()
            feed = LeaseIssueFeed(config, changed, interval=60)
            worker = Thread(target=feed.run, daemon=True)
            with patch('lease_review.page') as fetch:
                fetch.side_effect = [
                    OSError('temporarily offline'),
                    {'counts': counts, 'as_of': time.time(), 'next_deadline': None,
                     'issues': [{'code': 'export-failed', 'task': 'Build'}]},
                    {'counts': counts, 'as_of': time.time(), 'next_deadline': None, 'issues': []},
                ]
                worker.start()
                try:
                    feed.refresh()
                    self.assertFalse(changed.wait(0.05))
                    fetch.assert_not_called()
                    with feed.watch():
                        self.assertTrue(changed.wait(2))
                        self.assertEqual(feed.snapshot()['status'], 'unavailable')
                        changed.clear()
                        with feed.watch():
                            self.assertFalse(changed.wait(0.05))
                            self.assertEqual(fetch.call_count, 1)
                        feed.refresh()
                        self.assertTrue(changed.wait(2))
                        self.assertEqual(feed.snapshot()['issues'], [{'code':'export-failed','task':'Build'}])
                        self.assertEqual(feed.snapshot()['counts'], counts)
                        changed.clear()
                        feed.refresh()
                        self.assertTrue(changed.wait(2))
                        self.assertEqual(feed.snapshot()['issues'], [])
                    changed.clear()
                    feed.refresh()
                    self.assertFalse(changed.wait(0.05))
                    self.assertEqual(fetch.call_count, 3)
                finally:
                    feed.close()
                    worker.join(2)
