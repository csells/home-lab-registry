import json
import sqlite3
import sys
import tarfile
import tempfile
import time
import unittest
from unittest.mock import patch

from checkpoints import Rejected
from exporter import ExportError, collect, store
from pilot import Pilot
from review import lease_page


class SafetyTest(unittest.TestCase):
    def setUp(self):
        cfg = {
            "database": ":memory:",
            "client_tokens": {},
            "proxmox": {},
            "capacity": {},
            "guest_user": "tester",
            "guest_host_key": "",
            "reclamation": {"verifier_token_file": "ref", "purge_expired_checkpoints": True},
        }
        with (
            patch("pilot.Proxmox"),
            patch("pilot.Capacity"),
            patch("pilot.public_key", return_value="key"),
        ):
            self.controller = Pilot(cfg)
        self.c = self.controller
        self.addCleanup(self.c.db.close)
        self.c.provider.status.return_value = "stopped"
        self.c.provider.generation.return_value = "current"
        self.c.provider.recent_tasks.return_value = set()
        self.c.provider.start.return_value = "start"
        self.c.provider.matches_task.return_value = True

    def lease(self, key="new"):
        with patch("pilot.public_key", return_value="key"):
            return self.c.acquire({"task": "test", "request_key": key, "seconds": 60}, "owner")["id"]

    def test_stale_generation_cannot_stop_or_rekey_guest(self):
        lease = self.lease()
        with self.c.db:
            self.c.db.execute("UPDATE lease SET phase='ready',deadline=0")
        self.c.provider.generation.return_value = "replacement"
        self.c.provider.status.return_value = "running"
        with self.assertRaisesRegex(Rejected, "generation"):
            self.c.advance_reclamation()
        self.c.provider.shutdown.assert_not_called()
        with self.c.db:
            self.c.db.execute("UPDATE lease SET phase='starting'")
        with self.assertRaisesRegex(Rejected, "generation"):
            self.c.inspect(lease, "owner")
        self.c.provider.install_guest_key.assert_not_called()

    def test_ready_rechecks_ip_and_power(self):
        lease = self.lease()
        self.assertEqual(self.c.get(lease)["reclaim_on_expiry"], 1)
        with self.c.db:
            self.c.db.execute("UPDATE lease SET phase='ready', address='192.0.2.1'")
        self.c.provider.status.return_value = "running"
        self.c.provider.ip.return_value = "192.0.2.2"
        self.assertEqual(self.c.inspect(lease, "owner")["ssh"]["host"], "192.0.2.2")
        self.c.provider.status.return_value = "stopped"
        stopped = self.c.inspect(lease, "owner")
        self.assertEqual(stopped["status"], "guest-stopped")
        self.assertNotIn("ssh", stopped)

    def test_retry_keeps_storage_operation(self):
        lease = self.lease()
        with self.c.db:
            self.c.db.execute(
                "UPDATE lease SET phase='awaiting-export', export_failed_at=1, export_challenge='attempt',storage_challenge='original'"
            )
        with patch.object(self.c, "check_export_source"):
            self.c.retry_export(lease, "owner")
        self.assertEqual(self.c.get(lease)["storage_challenge"], "original")

    def test_restore_hold_covers_expiry_until_ack(self):
        source = self.lease()
        receipt = {
            "storage_managed": True,
            "verified_at": 1,
            "retain_through": 2,
            "sha256": "digest",
            "snapshot": "snapshot",
            "path": "/archive",
        }
        with self.c.db:
            self.c.db.execute(
                "UPDATE lease SET phase='awaiting-export',checkpoint=?", (json.dumps(receipt),)
            )
        self.assertEqual(lease_page(self.c.db, None)["leases"][0]["checkpoint_count"], 0)
        with self.c.db:
            self.c.db.execute("UPDATE lease SET phase='released'")
        new = self.lease("next")
        with self.c.db:
            self.c.db.execute("INSERT INTO restore_hold VALUES (?,?,NULL)", (new, source))
            self.c.db.execute(
                "UPDATE lease SET phase='ready',resume_point=? WHERE id=?",
                (json.dumps({"lease": source, "sha256": "digest"}), new),
            )
        self.assertIsNone(self.c.claim_expired_checkpoint()["expiry"])
        self.assertEqual(self.c.available_checkpoint(self.c.get(source), time.time(), None), receipt)
        self.assertEqual(lease_page(self.c.db, None, "released")["leases"][0]["checkpoint_count"], 1)
        self.c.finish_restore(new, "owner", {"sha256": "digest"})
        self.assertIsNotNone(self.c.claim_expired_checkpoint()["expiry"])
        with self.assertRaises(Rejected):
            self.c.available_checkpoint(self.c.get(source), time.time(), None)

    def test_unacknowledged_release_keeps_cleanup_identity(self):
        lease = self.lease()
        with self.c.db:
            self.c.db.execute(
                "UPDATE lease SET phase='released',released_at=1,export_challenge='attempt',storage_challenge='original'"
            )
        job = self.c.claim_expired_checkpoint()["expiry"]
        self.assertEqual(job["challenge"], "original")
        self.assertIsNone(job["snapshot"])
        self.c.confirm_checkpoint_expiry({**job, "absent": True})
        self.assertIsNone(self.c.claim_expired_checkpoint()["expiry"])

    def test_restore_budget_checked_before_storage(self):
        with tempfile.TemporaryFile() as archive:
            with tarfile.open(fileobj=archive, mode="w:gz") as tar:
                tar.addfile(tarfile.TarInfo("ordinary.txt"))
                unsupported = tarfile.TarInfo("pipe")
                unsupported.type = tarfile.FIFOTYPE
                tar.addfile(unsupported)
            with patch("exporter.command_json") as storage:
                with self.assertRaisesRegex(ExportError, "unsupported archive entry"):
                    store({"restic_command": ["unused"]}, archive, "example", "example", 3)
                storage.assert_not_called()

    def test_stage_and_safe_subprocess_cause(self):
        with tempfile.TemporaryFile() as out, self.assertRaises(ExportError) as error:
            collect(
                [
                    sys.executable,
                    "-c",
                    "import sys;sys.stderr.write('secret: no space left');sys.exit(7)",
                ],
                out,
                100,
                5,
                stage="backup",
            )
        self.assertEqual(str(error.exception), "backup: storage full (exit 7).")
