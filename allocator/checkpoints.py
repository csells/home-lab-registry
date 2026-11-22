"""Durable identities and upload grants for incremental checkpoints."""
import hashlib
import json
import re
import secrets
import time
import uuid

UPLOAD_SECONDS = 300
MAX_ARCHIVE_BYTES = 64 * 1024 ** 2


class Rejected(Exception):
    def __init__(self, code, reason, status=409):
        self.code, self.reason, self.status = code, reason, status


def sha256(value):
    return isinstance(value, str) and re.fullmatch(r'[a-f0-9]{64}', value)


def object_name(lease, checkpoint=None):
    suffix = '-' + checkpoint if checkpoint else ''
    return f'lease-{lease}{suffix}.tar.gz'


def recoverable(receipt, phase, final, now, held=False):
    return bool(receipt and receipt.get('storage_managed') and (not final or phase == 'released')
                and not receipt.get('purge_claim') and not receipt.get('purged_at')
                and (held or receipt.get('pin') or receipt.get('retain_through', float('inf')) > now))


def restore_held(db, lease, checkpoint):
    return db.execute("SELECT 1 FROM restore_hold WHERE source=? AND checkpoint_id IS ? LIMIT 1",
                      (lease, checkpoint)).fetchone() is not None


class Checkpoints:
    def __init__(self, db):
        self.db = db
        with db:
            db.execute("""CREATE TABLE IF NOT EXISTS incremental_checkpoint (
                id TEXT PRIMARY KEY, lease_id TEXT NOT NULL, request_key TEXT NOT NULL,
                created_at REAL NOT NULL, size INTEGER NOT NULL, sha256 TEXT NOT NULL,
                token_hash TEXT, upload_expires REAL, redeemed INTEGER NOT NULL DEFAULT 0,
                receipt TEXT, error TEXT, UNIQUE(lease_id, request_key)
            )""")

    @staticmethod
    def public(row):
        receipt = json.loads(row['receipt'] or '{}')
        receipt.pop('purge_claim', None)
        return {'id': row['id'], 'requested_at': row['created_at'],
                'error': row['error'], 'size': row['size'], **receipt}

    def list(self, lease):
        return [self.public(row) for row in self.db.execute(
            'SELECT * FROM incremental_checkpoint WHERE lease_id=? ORDER BY created_at DESC, id DESC LIMIT 32',
            (lease,))]

    def grant(self, lease, data, endpoint):
        key, digest, size = data.get('request_key'), data.get('sha256'), data.get('size')
        if (not isinstance(key, str) or not 0 < len(key) <= 128 or not sha256(digest)
                or type(size) is not int or not 0 < size <= MAX_ARCHIVE_BYTES):
            raise Rejected('invalid_checkpoint', 'Supply a request key, digest and permitted archive size.', 400)
        now = time.time()
        row = self.db.execute('SELECT * FROM incremental_checkpoint WHERE lease_id=? AND request_key=?',
                              (lease, key)).fetchone()
        if row:
            if (row['sha256'], row['size']) != (digest, size):
                raise Rejected('checkpoint_key_conflict', 'This key already identifies different bytes.')
            if row['receipt']:
                return {'checkpoint': self.public(row)}
        if not endpoint:
            raise Rejected('checkpoint_unavailable', 'Incremental uploads are unavailable.', 503)
        if self.db.execute("""SELECT 1 FROM incremental_checkpoint
                              WHERE lease_id=? AND receipt IS NULL AND upload_expires>? LIMIT 1""",
                           (lease, now)).fetchone():
            raise Rejected('checkpoint_in_progress', 'Another upload grant is active. Inspect its status before retrying.')
        checkpoint = row['id'] if row else uuid.uuid4().hex
        token = secrets.token_urlsafe(32)
        token_hash = hashlib.sha256(token.encode('ascii')).hexdigest()
        with self.db:
            if not row:
                self.db.execute("""INSERT INTO incremental_checkpoint
                    (id, lease_id, request_key, created_at, sha256, size) VALUES (?,?,?,?,?,?)""",
                                (checkpoint, lease, key, now, digest, size))
            self.db.execute("""UPDATE incremental_checkpoint
                SET token_hash=?, upload_expires=?, redeemed=0, error=NULL WHERE id=?""",
                            (token_hash, now + UPLOAD_SECONDS, checkpoint))
        return {'id': checkpoint, 'endpoint': endpoint, 'ticket': token}

    def ticket(self, token, redeemed):
        if not isinstance(token, str) or not re.fullmatch(r'[a-zA-Z0-9_-]{43}', token):
            raise Rejected('ticket_unavailable', 'Upload ticket is unavailable.', 404)
        token_hash = hashlib.sha256(token.encode('ascii')).hexdigest()
        row = self.db.execute('SELECT * FROM incremental_checkpoint WHERE token_hash=?',
                              (token_hash,)).fetchone()
        if not row or row['redeemed'] != redeemed or row['upload_expires'] <= time.time():
            raise Rejected('ticket_unavailable', 'Upload ticket is unavailable.', 404)
        return row

    def redeem(self, token):
        row = self.ticket(token, redeemed=0)
        with self.db:
            self.db.execute('UPDATE incremental_checkpoint SET redeemed=1, upload_expires=? WHERE id=?',
                            (time.time() + UPLOAD_SECONDS, row['id']))
        return {'id': row['id'], 'size': row['size'], 'sha256': row['sha256'],
                'path': '/' + object_name(row['lease_id'], row['id'])}

    def complete(self, data, retention):
        row = self.ticket(data.get('ticket'), redeemed=1)
        snapshot, error = data.get('snapshot'), data.get('error')
        if error is not None:
            if not isinstance(error, str) or not 0 < len(error) <= 240 or row['receipt']:
                raise Rejected('invalid_checkpoint_result', 'Invalid upload failure.', 400)
            with self.db:
                self.db.execute('''UPDATE incremental_checkpoint SET token_hash=NULL, upload_expires=NULL, error=? WHERE id=?''',
                                (error, row['id']))
        else:
            if not sha256(snapshot) or data.get('sha256') != row['sha256']:
                raise Rejected('invalid_checkpoint_result', 'Upload did not verify the declared bytes.', 400)
            if row['receipt']:
                if json.loads(row['receipt'])['snapshot'] != snapshot:
                    raise Rejected('checkpoint_result_conflict', 'A different snapshot was already acknowledged.')
            else:
                receipt = {'snapshot': snapshot, 'path': '/' + object_name(row['lease_id'], row['id']),
                           'sha256': row['sha256'], 'verified_at': time.time(), 'storage_managed': True}
                if retention is not None:
                    receipt['retain_through'] = retention
                self.save(row['id'], receipt)
        return self.public(self.get(row['lease_id'], row['id']))

    def get(self, lease, checkpoint):
        row = self.db.execute('SELECT * FROM incremental_checkpoint WHERE lease_id=? AND id=?',
                              (lease, checkpoint)).fetchone()
        if row is None:
            raise Rejected('checkpoint_unavailable', 'No such checkpoint is available.', 404)
        return row

    def save(self, checkpoint, receipt):
        with self.db:
            self.db.execute('UPDATE incremental_checkpoint SET receipt=?, error=NULL WHERE id=?',
                            (json.dumps(receipt), checkpoint))
