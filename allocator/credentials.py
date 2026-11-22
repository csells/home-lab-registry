"""Short-lived task credentials minted by a configured orchestration principal."""
import hashlib
import re
import secrets
import time

from checkpoints import Rejected


class TaskCredentials:
    def __init__(self, db, maximum_seconds):
        if type(maximum_seconds) is not int or maximum_seconds < 1:
            raise ValueError('Configure a positive task credential lifetime')
        self.db, self.maximum_seconds = db, maximum_seconds
        with db:
            db.execute("""CREATE TABLE IF NOT EXISTS task_credential (
                owner TEXT PRIMARY KEY, token_hash TEXT UNIQUE NOT NULL,
                expires_at REAL NOT NULL, revoked_at REAL
            )""")

    @staticmethod
    def token_hash(token):
        return hashlib.sha256(token.encode('ascii')).hexdigest()

    def owner(self, token):
        if not re.fullmatch(r'[a-zA-Z0-9_-]{43}', token):
            return None
        row = self.db.execute("""SELECT owner FROM task_credential
                            WHERE token_hash=? AND revoked_at IS NULL AND expires_at>?""",
                              (self.token_hash(token), time.time())).fetchone()
        return row['owner'] if row else None

    def issue(self, issuer, data):
        task = data.get('task_ref')
        if not isinstance(task, str) or not re.fullmatch(r'[a-zA-Z0-9_-]{1,96}', task):
            raise Rejected('invalid_task_ref', 'Supply a task reference for this issuer.', 400)
        owner = f'issued:{issuer}:{task}'
        now = time.time()
        if data.get('revoke') is True:
            with self.db:
                self.db.execute('UPDATE task_credential SET revoked_at=coalesce(revoked_at, ?) WHERE owner=?',
                                (now, owner))
                self.db.execute('''DELETE FROM renewal_delegate WHERE lease_id IN (SELECT id FROM lease WHERE owner=?)''',
                                (owner,))
            return {'revoked': True, 'principal': owner}

        duration = data.get('seconds')
        if type(duration) is not int or not 0 < duration <= self.maximum_seconds:
            raise Rejected('invalid_credential_duration',
                           f'Task access must be between 1 and {self.maximum_seconds} seconds.', 400)
        token = secrets.token_urlsafe(32)
        expiry = now + duration
        with self.db:
            self.db.execute("""INSERT INTO task_credential VALUES (?,?,?,NULL)
                ON CONFLICT(owner) DO UPDATE SET token_hash=excluded.token_hash,
                expires_at=excluded.expires_at, revoked_at=NULL""",
                            (owner, self.token_hash(token), expiry))
        return {'token': token, 'principal': owner, 'expires_at': expiry}
