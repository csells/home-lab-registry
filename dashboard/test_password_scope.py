import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch
from access import credential


class PasswordScope(unittest.TestCase):
    def test_extra_credentials_are_denied_at_lookup(self):
        with (
            tempfile.TemporaryDirectory() as directory,
            patch.dict(os.environ, {"LAB_DASHBOARD_ACCESS_TARGET": "target"}),
        ):
            private = Path(directory)
            (private / "machine-passwords.json").write_text(
                json.dumps(
                    {
                        "target": {"console": "example", "unused": "example"},
                        "other": {"console": "example"},
                    }
                )
            )
            machines = [{"id": "target", "access": [{"id": "console", "password_available": True}]}]
            self.assertIsNone(credential(private, "other", "console", machines))
            self.assertIsNone(credential(private, "target", "unused", machines))
            self.assertIsNone(credential(private, "target", "console", []))
            self.assertEqual(credential(private, "target", "console", machines), "example")
