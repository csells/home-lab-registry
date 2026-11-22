"""Detect old database and whole-controller restores before lease mutations."""
import fcntl
import json
import os
import sqlite3
import tempfile
import uuid
from pathlib import Path

from checkpoints import Rejected


class Ledger(sqlite3.Connection):
    witness = None

    def attach(self, location, generation, *, enroll=False):
        self.witness = Path(location)
        self.controller_generation = generation
        self.lock = self.witness.with_suffix(self.witness.suffix + '.lock').open('a')
        fcntl.flock(self.lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        if enroll:
            if self.witness.exists() or self.execute("SELECT 1 FROM sqlite_master WHERE name='ledger_revision'").fetchone():
                raise ValueError('A ledger or witness is already enrolled. Reconcile it before recovery.')
            self.execute('CREATE TABLE ledger_revision (id INTEGER PRIMARY KEY CHECK(id=1), revision TEXT NOT NULL)')
            revision = uuid.uuid4().hex
            self.execute('INSERT INTO ledger_revision VALUES (1,?)', (revision,))
            self.write_witness(revision, generation())
            self.commit()
        self.verify()

    def write_witness(self, revision, generation):
        with tempfile.NamedTemporaryFile(dir=self.witness.parent, mode='w', delete=False) as f:
            json.dump({'revision': revision, 'controller_generation': generation}, f)
            f.flush()
            os.fsync(f.fileno())
        os.replace(f.name, self.witness)
        parent = os.open(self.witness.parent, os.O_RDONLY)
        try:
            os.fsync(parent)
        finally:
            os.close(parent)

    def verify(self):
        if self.witness is None:
            return
        if not self.witness.is_file():
            raise Rejected('ledger_reconciliation_required', 'Missing independent ledger witness.')
        seal = json.loads(self.witness.read_text())
        row = self.execute('SELECT revision FROM ledger_revision WHERE id=1').fetchone()
        if not row or row[0] != seal['revision'] or self.controller_generation() != seal['controller_generation']:
            raise Rejected('ledger_reconciliation_required', 'Ledger revision or controller VM generation differs. Reconcile offline.')
        return seal['controller_generation']

    def __exit__(self, typ, value, trace):
        if typ is None and self.in_transaction and self.witness is not None:
            try:
                generation = self.verify()
                revision = uuid.uuid4().hex
                self.execute('UPDATE ledger_revision SET revision=? WHERE id=1', (revision,))
                # A crash between fsync and commit leaves a mismatch and requires reconciliation.
                self.write_witness(revision, generation)
            except BaseException:
                self.rollback()
                raise
        return super().__exit__(typ, value, trace)

    def close(self):
        try:
            super().close()
        finally:
            if hasattr(self, 'lock'):
                self.lock.close()
