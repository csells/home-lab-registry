"""Regression checks for restoring a task archive without losing later edits."""
import hashlib
import io
import json
import os
import subprocess
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch
from restore_workspace import extract_workspace


class WorkspaceRestoreTest(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.home = Path(tmp.name)
        data = io.BytesIO()
        with tarfile.open(fileobj=data, mode='w:gz') as archive:
            info = tarfile.TarInfo('work.txt')
            info.size = 5
            archive.addfile(info, io.BytesIO(b'hello'))
        self.archive = data.getvalue()
        self.digest = hashlib.sha256(self.archive).hexdigest()
        self.lease = '1' * 32
        self.workspace = self.home / 'workspace'

    def recover(self, digest=None):
        return subprocess.run([sys.executable, str(Path(__file__).with_name('restore_workspace.py')),
                               digest or self.digest, self.lease],
                              env={**os.environ, 'HOME': str(self.home)}, input=self.archive,
                              capture_output=True, timeout=5)

    def test_replay_preserves_edits(self):
        first = self.recover()
        self.assertEqual(first.returncode, 0, first.stderr)
        self.assertEqual((self.workspace/'work.txt').read_bytes(), b'hello')
        (self.workspace/'work.txt').write_bytes(b'later work')
        replay = self.recover()
        self.assertEqual(replay.stdout.strip(), b'previously-restored')
        self.assertEqual((self.workspace/'work.txt').read_bytes(), b'later work')
        self.assertEqual(json.loads((self.workspace/'.lease-restore-receipt').read_text())['lease'], self.lease)

    def test_tar_metadata_counts_toward_expanded_budget(self):
        source = io.BytesIO()
        with tarfile.open(fileobj=source, mode='w:gz', format=tarfile.PAX_FORMAT) as archive:
            entry = tarfile.TarInfo('file.txt')
            entry.pax_headers = {'comment': 'ordinary metadata' * 200}
            archive.addfile(entry)
        source.seek(0)
        with patch('restore_workspace.MAX_EXPANDED', 1024):
            with self.assertRaisesRegex(ValueError, 'workspace budget'):
                extract_workspace(source, self.home)
        self.assertEqual(list(self.home.iterdir()), [])

    def test_other_workspace_and_bad_digest_are_preserved(self):
        self.assertNotEqual(self.recover('0'*64).returncode, 0)
        self.assertFalse(self.workspace.exists())
        self.workspace.mkdir()
        (self.workspace/'existing').write_text('mine')
        self.assertNotEqual(self.recover().returncode, 0)
        self.assertEqual(list(self.workspace.iterdir()), [self.workspace/'existing'])
        self.assertEqual(list(self.home.iterdir()), [self.workspace])


if __name__ == '__main__':
    unittest.main()
