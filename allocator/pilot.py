"""Private, single-slot Linux lease pilot. Run on loopback in a disposable VM."""
import base64
import binascii
import hmac
import json
import re
import sqlite3
import ssl
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path


class Rejected(Exception):
    def __init__(self, code, reason, status=409):
        self.code, self.reason, self.status = code, reason, status


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, fp, code, message, headers, newurl):
        return None


EXPORT_REVIEW_SECONDS = 24 * 60 * 60
CHECKPOINT_RETENTION_SECONDS = 30 * 24 * 60 * 60


PROVIDER_TASK_TYPES = {
    'starting': 'qmstart',
    'stopping': 'qmshutdown',
    'export-stopping': 'qmshutdown',
    'resetting': 'qmrollback',
}


def public_key(value):
    parts = value.split() if isinstance(value, str) and len(value) <= 200 else []
    if len(parts) == 2 and parts[0] == 'ssh-ed25519':
        try:
            blob = base64.b64decode(parts[1], validate=True)
        except binascii.Error:
            blob = b''
        prefix = bytes.fromhex('0000000b') + b'ssh-ed25519' + bytes.fromhex('00000020')
        if len(blob) == len(prefix) + 32 and blob.startswith(prefix):
            return 'ssh-ed25519 ' + base64.b64encode(blob).decode('ascii')
    raise Rejected('invalid_ssh_key', 'Provide one OpenSSH Ed25519 public key without a comment.', 400)


class Proxmox:
    def __init__(self, config):
        self.base = config['url'].rstrip('/') + '/api2/json'
        url = urllib.parse.urlsplit(config['url'])
        if (url.scheme != 'https' or not url.hostname or url.username or
                url.password or url.path not in ('', '/') or url.query or url.fragment):
            raise ValueError('Proxmox requires HTTPS')
        self.node = config['node']
        self.guest_id = str(int(config['guest_id']))
        self.guest = f'/nodes/{self.node}/qemu/{self.guest_id}'
        self.token = Path(config['token_file']).read_text().strip()
        self.identity = self.token.split('=', 1)[0]
        if '!' not in self.identity:
            raise ValueError('Use a dedicated API token for the pilot slot')
        self.context = ssl.create_default_context(cafile=config['ca_file'])
        self.opener = urllib.request.build_opener(
            urllib.request.ProxyHandler({}), urllib.request.HTTPSHandler(context=self.context), NoRedirect())
        self.name, self.snapshot = config['guest_name'], config['snapshot']

    def call(self, route, fields=None, *, token=None):
        data = None if fields is None else urllib.parse.urlencode(fields).encode()
        request = urllib.request.Request(self.base + route, data=data,
                                         headers={'Authorization': 'PVEAPIToken=' + (token or self.token)})
        with self.opener.open(request, timeout=5) as response:
            return json.load(response)['data']

    def status(self):
        config = self.call(self.guest + '/config')
        if config['name'] != self.name or config.get('template'):
            raise Rejected('slot_identity_changed', 'The configured guest no longer matches this pilot slot.')
        if config.get('lock'):
            raise Rejected('slot_locked', 'Proxmox reports an in-flight task on the pilot slot.')
        return self.call(self.guest + '/status/current')['status']

    def start(self):
        return self.call(self.guest + '/status/start', {})

    def shutdown(self):
        return self.call(self.guest + '/status/shutdown', {'timeout': 20})

    def reset(self):
        snap = urllib.parse.quote(self.snapshot, safe='')
        return self.call(self.guest + '/snapshot/' + snap + '/rollback', {'start': 0})

    def matches_task(self, upid, phase):
        parts = upid.split(':') if isinstance(upid, str) else []
        return (len(parts) == 9 and parts[0] == 'UPID' and parts[1] == self.node
                and parts[5:8] == [PROVIDER_TASK_TYPES[phase], self.guest_id, self.identity])

    def slot_start_tasks(self):
        query = urllib.parse.urlencode({
            'source': 'all', 'vmid': self.guest_id, 'typefilter': 'qmstart', 'limit': 50,
        })
        tasks = self.call(f'/nodes/{self.node}/tasks?{query}')
        return json.dumps(sorted(row['upid'] for row in tasks))

    def recent_tasks(self, phase):
        query = urllib.parse.urlencode({
            'source': 'all', 'vmid': self.guest_id,
            'typefilter': PROVIDER_TASK_TYPES[phase], 'limit': 50,
        })
        tasks = self.call(f'/nodes/{self.node}/tasks?{query}')
        return {row['upid'] for row in tasks if self.matches_task(row.get('upid'), phase)}

    def task(self, upid):
        path = f'/nodes/{self.node}/tasks/{urllib.parse.quote(upid, safe="")}/status'
        result = self.call(path)
        if result['status'] == 'running':
            return False
        if result.get('exitstatus') != 'OK':
            raise Rejected('provider_task_failed', 'The recorded provider task failed and needs operator review.')
        return True

    def install_guest_key(self, key):
        self.call(self.guest + '/agent/file-write', {
            'file': '/etc/ssh/lease_authorized_keys',
            'content': key + '\n',
        })

    def ip(self):
        interfaces = self.call(self.guest + '/agent/network-get-interfaces')['result']
        for interface in interfaces:
            if interface['name'] == 'lo':
                continue
            for address in interface.get('ip-addresses', []):
                if address.get('ip-address-type') == 'ipv4':
                    return address['ip-address']
        return None


MIB = 1024 ** 2


class Capacity:
    def __init__(self, provider, config):
        self.provider = provider
        self.token = Path(config['token_file']).read_text().strip()
        if not self.token or self.token.split('=', 1)[0] == provider.identity:
            raise ValueError('Use a separate read-only Proxmox capacity token')
        self.storage = config['storage']
        self.memory_floor = self.bytes(config['minimum_memory_mib'])
        self.storage_floor = self.bytes(config['minimum_storage_mib'])
        self.reserved_memory = self.bytes(config['other_reserved_memory_mib'])
        self.protected_ids = {str(int(value)) for value in config['protected_guest_ids']}
        if not self.protected_ids or self.provider.guest_id in self.protected_ids:
            raise ValueError('List protected guests separately from the pilot slot')

    @staticmethod
    def bytes(value):
        if type(value) is not int or value < 0:
            raise ValueError('Capacity values must be nonnegative MiB integers')
        return value * MIB

    def check(self):
        try:
            observation = self.observe()
        except (OSError, ValueError, TypeError, KeyError) as error:
            print(f'Capacity observation failed: {type(error).__name__}', flush=True)
            raise Rejected('capacity_unverified', 'Fresh host capacity could not be verified. Try again later.', 503) from error

        for value, minimum, code, label in (
            (observation['memory_after_start'], self.memory_floor, 'host_memory_headroom', 'memory'),
            (observation['storage_after_writes'], self.storage_floor, 'host_storage_headroom', 'storage'),
        ):
            if value < minimum:
                raise Rejected(code, f'Starting this slot would leave {max(0, value) // MIB} MiB of host {label} headroom; {minimum // MIB} MiB is required.')

    def observe(self):
        node = '/nodes/' + urllib.parse.quote(self.provider.node, safe='')
        def read(path):
            return self.provider.call(path, token=self.token)
        memory = read(node + '/status')['memory']
        rows = read('/cluster/resources?type=vm')
        storage = read(node + '/storage/' + urllib.parse.quote(self.storage, safe='') + '/status')
        if not isinstance(rows, list) or any(not isinstance(row, dict) for row in rows):
            raise ValueError('Invalid Proxmox resource inventory')
        guests = {}
        for row in rows:
            if row.get('node') != self.provider.node:
                continue
            guest_id = str(row['vmid'])
            if guest_id in guests or row['status'] not in ('running', 'stopped'):
                raise ValueError('Duplicate guest or unobservable guest state')
            guests[guest_id] = row
        if not self.protected_ids <= guests.keys():
            raise ValueError('Protected guests are missing from the capacity view')
        slot = guests[self.provider.guest_id]
        if slot['name'] != self.provider.name or slot['status'] != 'stopped' or slot.get('template'):
            raise ValueError('The capacity view does not match the stopped pilot guest')
        reserved = sum(self.positive(row['maxmem']) for row in guests.values()
                       if row['status'] == 'running' or str(row['vmid']) in self.protected_ids)
        total, available = self.positive(memory['total']), self.positive(memory['available'])
        if available > total or not storage['active'] or not storage['enabled']:
            raise ValueError('Invalid node memory or inactive slot storage')
        available_storage = self.positive(storage['avail'])
        return {
            'memory_after_start': min(available, total - reserved)
            - self.reserved_memory - self.positive(slot['maxmem']),
            'storage_after_writes': available_storage - self.positive(slot['maxdisk']),
        }

    @staticmethod
    def positive(value):
        if type(value) is not int or value <= 0:
            raise ValueError('Expected a positive byte count')
        return value


class Pilot:
    def __init__(self, config):
        self.db = sqlite3.connect(config['database'])
        self.db.row_factory = sqlite3.Row
        self.db.execute('PRAGMA journal_mode=WAL')
        self.db.execute('PRAGMA synchronous=FULL')
        self.db.execute('PRAGMA busy_timeout=5000')
        self.db.execute('''CREATE TABLE IF NOT EXISTS lease (
            id TEXT PRIMARY KEY, request_key TEXT UNIQUE NOT NULL,
            task TEXT NOT NULL, initial_seconds INTEGER NOT NULL,
            phase TEXT NOT NULL, deadline REAL NOT NULL, upstream TEXT, address TEXT
        )''')
        self.db.execute("""CREATE UNIQUE INDEX IF NOT EXISTS one_reserved_slot
            ON lease ((1)) WHERE phase != 'released'""")
        columns = {row['name'] for row in self.db.execute('PRAGMA table_info(lease)')}
        with self.db:
            for column, definition in (
                ('prior_tasks', 'TEXT'),
                ('owner', "TEXT NOT NULL DEFAULT 'operator'"),
                ('ssh_key', 'TEXT'),
                ('reclaim_on_expiry', 'INTEGER NOT NULL DEFAULT 0'),
                ('hold_mode', 'TEXT'),
                ('hold_reason', 'TEXT'),
                ('export_challenge', 'TEXT'),
                ('export_start_tasks', 'TEXT'),
                ('export_requested_at', 'REAL'),
                ('export_failed_at', 'REAL'),
                ('export_error', 'TEXT'),
                ('checkpoint', 'TEXT'),
            ):
                if column not in columns:
                    self.db.execute(f'ALTER TABLE lease ADD COLUMN {column} {definition}')
        self.provider = Proxmox(config['proxmox'])
        self.capacity = Capacity(self.provider, config['capacity'])
        self.user = config['guest_user']
        self.host_key = public_key(config['guest_host_key'])
        policy = config.get('reclamation', {})
        self.reclaim_enabled = bool(policy.get('verifier_token_file'))
        self.grace_seconds = policy.get('grace_seconds', 7200)
        if type(self.grace_seconds) is not int or self.grace_seconds < 1:
            raise ValueError('Grace must be a positive duration')

    def update(self, lease, phase, upstream=None, address=None, prior_tasks=None):
        with self.db:
            self.db.execute('UPDATE lease SET phase=?, upstream=?, address=?, prior_tasks=? WHERE id=?',
                            (phase, upstream, address, prior_tasks, lease))
        return self.get(lease)

    def get(self, lease):
        row = self.db.execute('SELECT * FROM lease WHERE id=?', (lease,)).fetchone()
        if row is None:
            raise Rejected('unknown_lease', 'No lease has this identifier.', 404)
        return dict(row)

    def owned(self, lease, owner):
        row = self.get(lease)
        if row['owner'] != owner:
            raise Rejected('unknown_lease', 'No lease has this identifier.', 404)
        return row

    def dispatch(self, lease, phase, operation, *, recorded=False):
        if not recorded:
            prior = json.dumps(sorted(self.provider.recent_tasks(phase)))
            self.update(lease, phase, prior_tasks=prior)
        # Dispatch once. Recovery can only attach a newly observed, matching
        # provider task to this intent. With zero or multiple matches, hold.
        task = operation()
        if not self.provider.matches_task(task, phase):
            raise Rejected('operation_outcome_unknown', 'Proxmox returned no matching task identifier. The slot remains held.')
        return self.update(lease, phase, upstream=task)

    def recover_task(self, row):
        if row['prior_tasks'] is None:
            return row
        prior = set(json.loads(row['prior_tasks']))
        new = self.provider.recent_tasks(row['phase']) - prior
        if len(new) != 1:
            return row
        return self.update(row['id'], row['phase'], upstream=new.pop())

    def inspect(self, lease, owner):
        row = self.owned(lease, owner)
        phase = row['phase']
        if phase in ('starting', 'stopping', 'export-stopping', 'resetting'):
            if not row['upstream']:
                row = self.recover_task(row)
                if not row['upstream']:
                    return self.public(row, 'outcome-unknown')
            if not self.provider.task(row['upstream']):
                return self.public(row, phase)
        if phase == 'starting':
            if self.provider.status() != 'running':
                raise Rejected('unexpected_guest_state', 'The provider completed start but the guest is not running.')
            try:
                address = self.provider.ip()
            except urllib.error.HTTPError as error:
                if error.code != 500:
                    raise
                address = None
            if not address:
                return self.public(row, 'waiting-for-guest-ip')
            if not row['ssh_key']:
                raise Rejected('missing_lease_key', 'The lease has no task SSH key. The slot remains held.')
            # Replaying this fixed-path write installs the same key for the
            # same lease. The guest is not handed out until it succeeds.
            self.provider.install_guest_key(row['ssh_key'])
            row = self.update(lease, 'ready', address=address)
        elif phase == 'stopping':
            if self.provider.status() != 'stopped':
                raise Rejected('unexpected_guest_state', 'The guest did not stop. The slot remains held.')
            row = self.dispatch(lease, 'resetting', self.provider.reset)
        elif phase == 'export-stopping':
            if self.provider.status() != 'stopped':
                raise Rejected('unexpected_guest_state', 'The guest did not stop. Preserve its disk.')
            row = self.request_export(lease)
        elif phase == 'resetting':
            if self.provider.status() != 'stopped':
                raise Rejected('unexpected_guest_state', 'The reset slot did not return to the stopped state.')
            if row['checkpoint']:
                receipt = json.loads(row['checkpoint'])
                receipt['retain_through'] = time.time() + CHECKPOINT_RETENTION_SECONDS
                with self.db:
                    self.db.execute(
                        "UPDATE lease SET phase='released', upstream=NULL, address=NULL, prior_tasks=NULL, checkpoint=? WHERE id=?",
                        (json.dumps(receipt), lease))
                row = self.get(lease)
            else:
                row = self.update(lease, 'released')
        status = row['phase']
        if status == 'ready' and time.time() >= row['deadline']:
            status = 'overdue'
        return self.public(row, status)

    def public(self, row, status=None):
        result = {key: value for key, value in row.items() if key not in ('prior_tasks', 'ssh_key', 'request_key', 'export_challenge', 'export_start_tasks', 'checkpoint')}
        result['status'] = status or row['phase']
        if row['checkpoint']:
            result['checkpoint'] = json.loads(row['checkpoint'])
        if row['export_failed_at']:
            result['export_needs_decision'] = time.time() >= row['export_failed_at'] + EXPORT_REVIEW_SECONDS
        if row['address']:
            result['ssh'] = {'host': row['address'], 'user': self.user,
                             'host_key_alias': self.provider.name, 'host_key': self.host_key}
        return result

    def acquire(self, data, owner):
        task, key, seconds = data.get('task'), data.get('request_key'), data.get('seconds')
        if not all(isinstance(s, str) and 0 < len(s) <= 128 for s in (task, key)):
            raise Rejected('invalid_request', 'Provide a task and a request_key of 1–128 characters.', 400)
        self.check_duration(seconds)
        reclaim = data.get('reclaim_on_expiry', False)
        if type(reclaim) is not bool or (reclaim and not self.reclaim_enabled):
            raise Rejected('reclamation_unavailable', 'Automatic reclamation is unavailable for this request.', 422)
        key = owner + '/' + key
        ssh_key = public_key(data.get('ssh_public_key'))
        if data.get('recipe') != 'debian13-amd64':
            raise Rejected('unsupported_recipe', 'The pilot provides only debian13-amd64.', 422)
        prev = self.db.execute('SELECT * FROM lease WHERE request_key=?', (key,)).fetchone()
        if prev:
            if prev['task'] != task or prev['initial_seconds'] != seconds or prev['ssh_key'] != ssh_key or bool(prev['reclaim_on_expiry']) != reclaim:
                raise Rejected('request_key_conflict', 'This request key is already bound to different inputs.')
            return self.inspect(prev['id'], owner)
        if self.db.execute("SELECT 1 FROM lease WHERE phase!='released'").fetchone():
            raise Rejected('capacity_exhausted', 'The single pilot slot is already reserved.')
        if self.provider.status() != 'stopped':
            raise Rejected('slot_unavailable', 'The pilot guest is already running outside the controller.')
        prior = json.dumps(sorted(self.provider.recent_tasks('starting')))
        self.capacity.check()
        lease = uuid.uuid4().hex
        with self.db:
            self.db.execute('''INSERT INTO lease
                (id, request_key, task, initial_seconds, phase, deadline, prior_tasks, owner, ssh_key, reclaim_on_expiry)
                VALUES (?,?,?,?,?,?,?,?,?,?)''',
                            (lease, key, task, seconds, 'starting', time.time()+seconds, prior, owner, ssh_key, int(reclaim)))
        return self.public(self.dispatch(lease, 'starting', self.provider.start, recorded=True))

    @staticmethod
    def check_duration(seconds):
        if type(seconds) is not int or not 0 < seconds <= 86400 * 365:
            raise Rejected('invalid_duration', 'Provide a duration between one second and one year.', 400)

    def renew(self, lease, data, owner):
        seconds = data.get('seconds')
        self.check_duration(seconds)
        row = self.owned(lease, owner)
        if row['phase'] not in ('starting', 'ready'):
            raise Rejected('lease_not_renewable', 'Only starting or ready leases can renew.')
        with self.db:
            self.db.execute('UPDATE lease SET deadline=? WHERE id=?',
                            (max(row['deadline'], time.time()+seconds), lease))
        return self.inspect(lease, owner)

    def hold(self, lease, data, owner):
        row = self.owned(lease, owner)
        mode, reason = data.get('mode', 'disk'), data.get('reason')
        if not row['reclaim_on_expiry'] or row['phase'] not in ('ready', 'awaiting-export'):
            raise Rejected('hold_unavailable', 'Only an enrolled lease before reset can be held.')
        if mode not in ('running', 'disk', 'clear'):
            raise Rejected('invalid_hold', 'Select running, disk, or clear.', 400)
        if mode == 'running' and row['phase'] != 'ready':
            raise Rejected('already_stopped', 'The guest has already stopped for export.')
        if mode != 'clear' and (not isinstance(reason, str) or not 1 <= len(reason.strip()) <= 240):
            raise Rejected('hold_reason_required', 'Provide a short hold reason.', 400)
        with self.db:
            self.db.execute('UPDATE lease SET hold_mode=?, hold_reason=? WHERE id=?',
                            (None if mode == 'clear' else mode,
                             None if mode == 'clear' else reason.strip(), lease))
        return self.public(self.get(lease))

    def request_export(self, lease):
        start_tasks = self.provider.slot_start_tasks()
        with self.db:
            self.db.execute("""UPDATE lease SET phase='awaiting-export', upstream=NULL,
                prior_tasks=NULL, address=NULL, export_challenge=?, export_requested_at=?,
                export_start_tasks=? WHERE id=?""", (uuid.uuid4().hex, time.time(), start_tasks, lease))
        return self.get(lease)

    def retry_export(self, lease, owner):
        row = self.owned(lease, owner)
        if row['phase'] != 'awaiting-export' or row['checkpoint'] or not row['export_failed_at']:
            raise Rejected('export_retry_unavailable', 'Only a failed pending export can be retried.')
        self.check_export_source(row)
        with self.db:
            self.db.execute(
                'UPDATE lease SET export_challenge=?, export_failed_at=NULL, export_error=NULL WHERE id=?',
                (uuid.uuid4().hex, lease))
        return self.public(self.get(lease))

    def next_export(self):
        row = self.db.execute(
            "SELECT id FROM lease WHERE phase='awaiting-export' AND checkpoint IS NULL"
            " AND (export_failed_at IS NULL OR export_failed_at > ?)",
            (time.time() - EXPORT_REVIEW_SECONDS,)).fetchone()
        return {'export': self.pending_export(row['id']) if row is not None else None}

    def pending_export(self, lease):
        row = self.get(lease)
        if row['phase'] != 'awaiting-export' or row['checkpoint']:
            raise Rejected('export_not_pending', 'This lease is not waiting for a final export.')
        if row['export_failed_at'] and time.time() >= row['export_failed_at'] + EXPORT_REVIEW_SECONDS:
            raise Rejected('export_decision_required', 'Export has failed for 24 hours. Owner disposition is required.')
        return {'lease_id': lease, 'challenge': row['export_challenge'],
                'guest_id': self.provider.guest_id, 'requested_at': row['export_requested_at']}

    def check_export_source(self, row):
        if (self.provider.status() != 'stopped' or
                self.provider.slot_start_tasks() != row['export_start_tasks']):
            raise Rejected('export_source_changed', 'The guest start history or power state changed. Preserve its disk for review.')

    def export_result(self, lease, data):
        row = self.get(lease)
        if row['checkpoint']:
            receipt = json.loads(row['checkpoint'])
            if (data.get('outcome') == 'verified' and data.get('challenge') == row['export_challenge']
                    and all(data.get(field) == receipt[field] for field in ('snapshot', 'path', 'sha256'))):
                return self.public(row)
            raise Rejected('export_already_verified', 'A different receipt cannot replace a verified export.')
        pending = self.pending_export(lease)
        if data.get('challenge') != pending['challenge']:
            raise Rejected('export_identity_mismatch', 'Receipt did not match the current export challenge.')
        self.check_export_source(row)
        if data.get('outcome') == 'failed':
            reason = data.get('reason')
            if not isinstance(reason, str) or not 1 <= len(reason.strip()) <= 240:
                raise Rejected('invalid_export_error', 'Provide a short failure reason.', 400)
            with self.db:
                self.db.execute("""UPDATE lease SET export_failed_at=coalesce(export_failed_at, ?),
                    export_error=? WHERE id=?""", (time.time(), reason.strip(), lease))
            print(f'Final export failed for lease {lease}; disk preserved', flush=True)
            return self.public(self.get(lease))
        snapshot, digest, path = data.get('snapshot'), data.get('sha256'), data.get('path')
        if (data.get('outcome') != 'verified' or not isinstance(snapshot, str)
                or not re.fullmatch(r'[a-f0-9]{64}', snapshot)
                or path != '/lease-' + lease + '.tar.gz'
                or not isinstance(digest, str) or not re.fullmatch(r'[a-f0-9]{64}', digest)):
            raise Rejected('invalid_export_receipt', 'Provide a verified encrypted snapshot and download digest.', 400)
        receipt = {'snapshot': snapshot, 'path': path, 'sha256': digest, 'verified_at': time.time()}
        with self.db:
            self.db.execute('UPDATE lease SET checkpoint=?, export_failed_at=NULL, export_error=NULL WHERE id=?',
                            (json.dumps(receipt), lease))
        return self.public(self.get(lease))

    def advance_reclamation(self):
        if not self.reclaim_enabled:
            return
        active = self.db.execute("SELECT * FROM lease WHERE phase!='released' AND reclaim_on_expiry=1").fetchone()
        if active is None:
            return
        row = dict(active)
        key, phase = row['id'], row['phase']
        if phase == 'starting':
            self.inspect(key, row['owner'])
        elif phase == 'ready' and time.time() >= row['deadline'] + self.grace_seconds and row['hold_mode'] != 'running':
            if self.provider.status() == 'stopped':
                self.request_export(key)
            else:
                self.dispatch(key, 'export-stopping', self.provider.shutdown)
        elif phase in ('export-stopping', 'resetting'):
            self.inspect(key, row['owner'])
        elif phase == 'awaiting-export' and row['checkpoint'] and not row['hold_mode']:
            self.check_export_source(row)
            self.dispatch(key, 'resetting', self.provider.reset)

    def release(self, lease, data, owner):
        row = self.owned(lease, owner)
        if row['phase'] in ('released', 'stopping', 'export-stopping', 'awaiting-export', 'resetting'):
            return self.inspect(lease, owner)
        if row['phase'] != 'ready':
            raise Rejected('lease_not_releasable', 'Inspect the start operation before requesting release.')
        if row['reclaim_on_expiry']:
            if data.get('preserve_checkpoint') is not True or data.get('discard_disposable_data'):
                raise Rejected('final_export_required', 'Enrolled leases require verified export before reset.')
            if row['hold_mode']:
                raise Rejected('lease_held', 'Clear the review hold before releasing the guest.')
            if self.provider.status() == 'stopped':
                return self.public(self.request_export(lease))
            return self.public(self.dispatch(lease, 'export-stopping', self.provider.shutdown))
        if data.get('discard_disposable_data') is not True:
            raise Rejected('release_requires_disposition', 'Confirm discard of the pilot workspace after saving any required data.', 400)
        if self.provider.status() == 'stopped':
            return self.public(self.dispatch(lease, 'resetting', self.provider.reset))
        return self.public(self.dispatch(lease, 'stopping', self.provider.shutdown))


def main():
    import argparse
    parser = argparse.ArgumentParser()
    parser.add_argument('config', type=Path)
    config = json.loads(parser.parse_args().config.read_text())
    controller = Pilot(config)
    secrets = {owner: Path(path).read_text().strip()
               for owner, path in config['client_tokens'].items()}
    if (not secrets or any(not re.fullmatch(r'[a-zA-Z0-9_-]{1,64}', owner)
                           for owner in secrets)
            or any(not re.fullmatch(r'[a-zA-Z0-9_-]{32,128}', value)
                   for value in secrets.values())
            or len(set(secrets.values())) != len(secrets)):
        raise ValueError('Configure a different randomly generated token for each pilot principal')

    verifier_ref = config.get('reclamation', {}).get('verifier_token_file')
    verifier = Path(verifier_ref).read_text().strip() if verifier_ref else None
    if verifier is not None and (not re.fullmatch(r'[a-zA-Z0-9_-]{32,128}', verifier)
                                 or verifier in secrets.values()):
        raise ValueError('Use a distinct final export verifier credential')

    class CheckedServer(HTTPServer):
        next_check = 0
        last_warning = None

        def get_request(self):
            connection, address = super().get_request()
            connection.settimeout(4)
            return connection, address

        def service_actions(self):
            if time.monotonic() < self.next_check:
                return
            self.next_check = time.monotonic() + 2
            try:
                controller.advance_reclamation()
            except (Rejected, OSError, ValueError, KeyError, TypeError) as error:
                warning = getattr(error, 'code', type(error).__name__)
                if warning != self.last_warning:
                    print(f'Reclamation held for review: {warning}', flush=True)
                self.last_warning = warning
            else:
                self.last_warning = None

    class Handler(BaseHTTPRequestHandler):
        def respond(self, value, status=200):
            body = json.dumps(value).encode()
            self.send_response(status)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Cache-Control', 'no-store')
            self.send_header('Content-Length', str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def route(self):
            supplied = self.headers.get('Authorization', '').encode('latin-1')
            owner = None
            for principal, value in secrets.items():
                if hmac.compare_digest(supplied, ('Bearer ' + value).encode('ascii')):
                    owner = principal
            is_verifier = verifier is not None and hmac.compare_digest(supplied, ('Bearer ' + verifier).encode('ascii'))
            parts = self.path.strip('/').split('/')
            export_route = len(parts) in (2, 3) and parts[:2] == ['v1', 'final-export']
            if (export_route and not is_verifier) or (not export_route and owner is None):
                self.respond({'code': 'unauthorized'}, 401)
                return
            try:
                if export_route and self.command == 'GET':
                    if len(parts) == 2:
                        self.respond(controller.next_export())
                    else:
                        self.respond(controller.pending_export(parts[2]))
                    return
                if self.command == 'GET' and len(parts) == 3 and parts[:2] == ['v1', 'leases']:
                    self.respond(controller.inspect(parts[2], owner))
                    return
                if self.command == 'POST':
                    size = int(self.headers.get('Content-Length', '0'))
                    if not 0 < size <= 4096:
                        raise Rejected('invalid_body', 'Provide a JSON object up to 4096 bytes.', 400)
                    data = json.loads(self.rfile.read(size))
                    if not isinstance(data, dict):
                        raise Rejected('invalid_body', 'Provide a JSON object.', 400)
                    if export_route and len(parts) == 3:
                        self.respond(controller.export_result(parts[2], data))
                        return
                    if parts == ['v1', 'leases']:
                        self.respond(controller.acquire(data, owner), 202)
                        return
                    if len(parts) == 4 and parts[:2] == ['v1', 'leases']:
                        if parts[3] == 'renew':
                            self.respond(controller.renew(parts[2], data, owner))
                            return
                        if parts[3] == 'hold':
                            self.respond(controller.hold(parts[2], data, owner))
                            return
                        if parts[3] == 'retry-export':
                            self.respond(controller.retry_export(parts[2], owner))
                            return
                        if parts[3] == 'release':
                            self.respond(controller.release(parts[2], data, owner), 202)
                            return
                self.respond({'code': 'unknown_route'}, 404)
            except Rejected as error:
                self.respond({'code': error.code, 'reason': error.reason}, error.status)
            except sqlite3.IntegrityError:
                self.respond({'code': 'lease_conflict', 'reason': 'The slot or request key is already reserved.'}, 409)
            except ValueError:
                self.respond({'code': 'invalid_body', 'reason': 'Provide valid JSON.'}, 400)
            except (OSError, urllib.error.URLError) as error:
                print(f'Provider request failed: {type(error).__name__}', flush=True)
                self.respond({'code': 'provider_unavailable', 'reason': 'Check the recorded lease before retrying.'}, 502)

        do_GET = route
        do_POST = route

    host = config.get('listen', '127.0.0.1')
    tls = config.get('tls')
    if host != '127.0.0.1' and not tls:
        raise ValueError('A non-loopback listener requires TLS')
    with CheckedServer((host, int(config.get('port', 8093))), Handler) as server:
        if tls:
            context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
            context.minimum_version = ssl.TLSVersion.TLSv1_3
            context.load_cert_chain(tls['cert_file'], tls['key_file'])
            server.socket = context.wrap_socket(server.socket, server_side=True, do_handshake_on_connect=False)
        print(f'Lease pilot listening with {"TLS" if tls else "local HTTP"}', flush=True)
        server.serve_forever()


if __name__ == '__main__':
    main()
