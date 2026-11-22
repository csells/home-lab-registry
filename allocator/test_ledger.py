import sqlite3
import tempfile
import unittest
from pathlib import Path

from checkpoints import Rejected
from ledger import Ledger


class LedgerTest(unittest.TestCase):
    def test_restart_stale_database_and_whole_vm_restore(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            database, witness = root / "authority.sqlite", root / "witness.json"
            db = sqlite3.connect(database, factory=Ledger)
            db.attach(witness, lambda: "controller-generation", enroll=True)
            with db:
                db.execute("CREATE TABLE policy (hold INTEGER)")
                db.execute("INSERT INTO policy VALUES (0)")
            old = sqlite3.connect(root / "old.sqlite")
            db.backup(old)
            old.close()
            with db:
                db.execute("UPDATE policy SET hold=1")
            db.close()
            current = sqlite3.connect(database, factory=Ledger)
            current.attach(witness, lambda: "controller-generation")
            self.assertEqual(current.execute("SELECT hold FROM policy").fetchone()[0], 1)
            current.close()
            for path, gen in [
                (root / "old.sqlite", "controller-generation"),
                (database, "new-controller-generation"),
            ]:
                stale = sqlite3.connect(path, factory=Ledger)
                try:
                    with self.assertRaisesRegex(Rejected, "Reconcile offline"):
                        stale.attach(witness, lambda: gen)
                finally:
                    stale.close()
