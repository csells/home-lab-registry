"""Owner-bound reservations for work that has not yet reached the hypervisor."""
import re
import time
import uuid

from checkpoints import Rejected


class Reservations:
    def __init__(self, db, check_capacity):
        self.db = db
        self.check_capacity = check_capacity
        with db:
            db.execute('BEGIN IMMEDIATE')
            db.execute("""CREATE TABLE IF NOT EXISTS capacity_reservation (
                id TEXT PRIMARY KEY, owner TEXT NOT NULL, request_key TEXT NOT NULL,
                memory_mib INTEGER NOT NULL, storage_mib INTEGER NOT NULL,
                purpose TEXT NOT NULL, created_at REAL NOT NULL, released_at REAL,
                UNIQUE(owner, request_key)
            )""")
            columns = {row['name'] for row in db.execute('PRAGMA table_info(capacity_reservation)')}
            if 'slot_id' not in columns:
                db.execute("ALTER TABLE capacity_reservation ADD COLUMN slot_id TEXT NOT NULL DEFAULT 'primary'")

    def totals(self, slot_id='primary'):
        return tuple(self.db.execute("""SELECT coalesce(sum(memory_mib), 0), coalesce(sum(storage_mib), 0)
            FROM capacity_reservation WHERE slot_id=? AND released_at IS NULL""", (slot_id,)).fetchone())

    def list(self, owner):
        return [dict(row) for row in self.db.execute("""SELECT * FROM capacity_reservation
                    WHERE owner=? AND released_at IS NULL ORDER BY created_at, id""", (owner,))]

    def reserve(self, owner, data):
        memory, storage = data.get('memory_mib'), data.get('storage_mib')
        key, purpose = data.get('request_key'), data.get('purpose')
        slot_id = data.get('slot_id', 'primary')
        if not isinstance(slot_id, str) or not re.fullmatch(r'[a-z][a-z0-9-]{0,31}', slot_id):
            raise Rejected('invalid_slot_id', 'Provide the configured slot identifier for this reservation.', 400)
        if (not all(type(value) is int and 0 <= value < 2**53 for value in (memory, storage))
                or not (memory or storage) or not isinstance(key, str) or not 1 <= len(key) <= 128
                or not isinstance(purpose, str) or not 1 <= len(purpose.strip()) <= 240):
            raise Rejected('invalid_reservation', 'Provide a key, purpose and nonzero memory or storage demand.', 400)
        row = self.db.execute('SELECT * FROM capacity_reservation WHERE owner=? AND request_key=?',
                              (owner, key)).fetchone()
        if row:
            if (row['memory_mib'], row['storage_mib'], row['purpose'], row['slot_id']) != (memory, storage, purpose.strip(), slot_id):
                raise Rejected('reservation_key_conflict', 'This key already identifies a different reservation.')
            return dict(row)

        reserved_memory, reserved_storage = self.totals(slot_id)
        self.check_capacity((reserved_memory + memory, reserved_storage + storage), slot_id)
        reservation = uuid.uuid4().hex
        with self.db:
            self.db.execute('''INSERT INTO capacity_reservation
                (id, owner, request_key, memory_mib, storage_mib, purpose, created_at, slot_id)
                VALUES (?,?,?,?,?,?,?,?)''',
                            (reservation, owner, key, memory, storage, purpose.strip(), time.time(), slot_id))
        return dict(self.get(owner, reservation))

    def get(self, owner, reservation):
        row = self.db.execute('SELECT * FROM capacity_reservation WHERE id=? AND owner=?',
                              (reservation, owner)).fetchone()
        if row is None:
            raise Rejected('reservation_unknown', 'No matching reservation for this principal.', 404)
        return row

    def release(self, owner, reservation):
        self.get(owner, reservation)
        with self.db:
            self.db.execute('UPDATE capacity_reservation SET released_at=coalesce(released_at, ?) WHERE id=?',
                            (time.time(), reservation))
        return dict(self.get(owner, reservation))
