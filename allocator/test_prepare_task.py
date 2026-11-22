import json
import os
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory
from unittest.mock import patch

from prepare_task import session_for_task


class LauncherTest(unittest.TestCase):
    def test_refresh_keeps_owner_and_ssh_identity(self):
        with TemporaryDirectory() as tmp:
            root = Path(tmp).resolve()
            ca = root / 'ca'
            ca.write_text('test certificate')
            settings = {'task_sessions': str(root), 'issuer_token_file': str(root / 'issuer'),
                        'ca_file': str(ca)}
            thread = 'independent-runner-owner'
            with (patch('prepare_task.discover_allocator', return_value='https://allocator.example'),
                  patch('prepare_task.request', side_effect=[
                      {'token': 'a' * 43, 'expires_at': 3600},
                      {'token': 'b' * 43, 'expires_at': 7100},
                  ]) as issue,
                  patch('prepare_task.time.time', return_value=0) as clock):
                session = session_for_task(settings, thread, 3600)
                key = (session.parent / 'identity').read_bytes()
                self.assertEqual(session_for_task(settings, thread, 3600), session)
                self.assertEqual(issue.call_count, 1)
                clock.return_value = 3500
                self.assertEqual(session_for_task(settings, thread, 3600), session)
                self.assertEqual((session.parent / 'identity').read_bytes(), key)
                self.assertEqual((session.parent / 'task.token').read_text(), 'b' * 43 + '\n')
                self.assertEqual(json.loads(session.read_text())['expires_at'], 7100)
                self.assertEqual(issue.call_args.args[-1]['task_ref'], thread)
                self.assertEqual(os.stat(session.parent).st_mode & 0o077, 0)
