import json
import sqlite3
import sys
import tarfile
import tempfile
import time
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

from checkpoints import Rejected
from exporter import ExportError, collect, export, store
from pilot import Pilot, Slot
from pve_dynamic import DynamicScope
from workspace_archive import archive_workspace
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

    def test_slot_ownership_survives_fallback_and_idempotent_retry(self):
        second = Mock()
        second.status.return_value = 'stopped'
        second.generation.return_value = 'second-generation'
        second.recent_tasks.return_value = set()
        second.start.return_value = 'second-start'
        second.matches_task.return_value = True
        capacity = Mock()
        observed = {'slot_resources': {}, 'memory_after_start': 4096, 'storage_after_writes': 8192}
        self.c.capacity.check.return_value = observed
        capacity.check.return_value = {**observed, 'memory_after_start': 8192}
        self.c.slots['second'] = Slot(second, capacity, 'second-user', 'second-key')

        first = self.lease('first')
        replacement = self.lease('second')
        self.assertEqual((self.c.get(first)['slot_id'], self.c.get(replacement)['slot_id']),
                         ('primary', 'second'))
        second.start.assert_called_once()
        self.c.provider.start.assert_called_once()
        with self.assertRaisesRegex(Rejected, 'No eligible slot'):
            self.lease('third')

        second.task.return_value = False
        self.assertEqual(self.lease('second'), replacement)
        second.task.assert_called_once_with('second-start')
        self.c.provider.task.assert_not_called()

        with self.c.db:
            self.c.db.execute("UPDATE lease SET phase='ready' WHERE id=?", (replacement,))
        second.status.return_value = 'running'
        second.shutdown.return_value = 'second-shutdown'
        self.c.release(replacement, {}, 'owner')
        second.shutdown.assert_called_once()
        self.c.provider.shutdown.assert_not_called()

    def test_capture_cannot_route_an_unenrolled_slot_to_primary(self):
        job = {'lease_id': 'a' * 32, 'challenge': 'b' * 32, 'storage_challenge': 'c' * 32,
               'guest_id': '97', 'slot_id': 'other'}
        with patch('exporter.collect') as capture:
            with self.assertRaisesRegex(ExportError, 'does not match'):
                export({'guest_id': '97', 'capture_command': ['primary-capture']}, job)
            capture.assert_not_called()

    def test_dynamic_capture_cannot_fall_back_to_a_fixed_command(self):
        job = {'lease_id': 'a' * 32, 'challenge': 'b' * 32, 'storage_challenge': 'c' * 32,
               'guest_id': '97', 'slot_id': 'primary',
               'dynamic_source': ['hardware', 'generation', 'boot-volume']}
        with patch('exporter.collect') as capture:
            with self.assertRaisesRegex(ExportError, 'cannot use a fixed'):
                export({'guest_id': '97', 'capture_command': ['fixed']}, job)
            capture.assert_not_called()

    def test_failed_physical_inventory_cannot_prove_a_guest_absent(self):
        provider = Mock(node='node', guest_id='97')
        provider.call.return_value = []
        scope = DynamicScope(provider, {
            'template_id': 98, 'template_name': 'template',
            'template_uuid': '3bd16a39-a08b-427d-a32f-aeaa6014444b',
            'template_volume': 'scope:98/base-98-disk-0.raw',
            'inventory_command': ['inventory'],
            'default': {'vcpus': 1, 'memory_mib': 512, 'disk_mib': 8192},
            'maximum': {'vcpus': 4, 'memory_mib': 4096, 'disk_mib': 8192},
        }, 'scope')
        with patch('pve_dynamic.subprocess.check_output', return_value=b'{}'):
            with self.assertRaisesRegex(Rejected, 'inventory was invalid'):
                scope.verify_absent()

    def test_archive_rejects_links_outside_workspace(self):
        with tempfile.TemporaryDirectory() as directory, tempfile.TemporaryFile() as archive:
            workspace = Path(directory).resolve() / 'workspace'
            workspace.mkdir()
            (workspace / 'outside').symlink_to('../outside')
            with self.assertRaisesRegex(ValueError, 'points outside'):
                archive_workspace(workspace, archive)

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

    def test_failed_validation_removes_partial_plaintext(self):
        extracted = []
        def interrupted(command, *args, **kwargs):
            partial = Path(command[-1]) / "partial"
            partial.write_bytes(b"task data")
            extracted.append(partial)
            raise ExportError("validator interrupted")

        with tempfile.TemporaryFile() as archive, patch("exporter.collect", side_effect=interrupted):
            archive.write(b"input")
            with self.assertRaisesRegex(ExportError, "interrupted"):
                store({"restic_command": ["unused"]}, archive, "file", "challenge", 5)
        self.assertFalse(extracted[0].parent.exists())

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
