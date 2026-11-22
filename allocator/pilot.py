"""Private, single-slot Linux lease pilot. Run on loopback in a disposable VM."""
import base64
import binascii
import hashlib
import hmac
import json
import re
import secrets
import sqlite3
import ssl
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path


from checkpoints import object_name, Checkpoints, Rejected, recoverable, restore_held
from capabilities import PLATFORM, RECIPE, match, requirements
from credentials import TaskCredentials
from ledger import Ledger
from reservations import Reservations
from review import lease_page


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, fp, code, message, headers, newurl):
        return None


EXPORT_REVIEW_SECONDS = 24 * 60 * 60
CHECKPOINT_RETENTION_SECONDS = 30 * 24 * 60 * 60
RECOVERY_TICKET_SECONDS = 300


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
        self.initial_rollback = config.get('initial_rollback')
        self.name, self.snapshot = config['guest_name'], config['snapshot']
        self.uuid, self.volume = config.get('guest_uuid'), config.get('guest_volume')
        if not self.uuid or not self.volume:
            raise ValueError('Pin both the slot hardware UUID and boot volume')

    def call(self, route, fields=None, *, token=None):
        data = None if fields is None else urllib.parse.urlencode(fields).encode()
        request = urllib.request.Request(self.base + route, data=data,
                                         headers={'Authorization': 'PVEAPIToken=' + (token or self.token)})
        with self.opener.open(request, timeout=5) as response:
            return json.load(response)['data']

    def verified_config(self):
        config = self.call(self.guest + '/config')
        if config['name'] != self.name or config.get('template'):
            raise Rejected('slot_identity_changed', 'The configured guest no longer matches this pilot slot.')
        if self.uuid and (('uuid=' + self.uuid) not in config.get('smbios1', '').split(',')
                          or config.get('scsi0', '').split(',')[0] != self.volume):
            raise Rejected('slot_identity_changed', 'The slot hardware UUID or boot volume changed.')
        if config.get('lock'):
            raise Rejected('slot_locked', 'Proxmox reports an in-flight task on the pilot slot.')
        return config

    def generation(self):
        raw = self.verified_config().get('vmgenid')
        if not isinstance(raw, str) or not re.fullmatch(r'[a-f0-9-]{36}', raw) or not uuid.UUID(raw).int:
            raise Rejected('slot_identity_changed', 'A provider VM generation ID is required.')
        return raw

    def audit_tasks(self, query):
        node = '/nodes/' + self.node
        permissions = self.call('/access/permissions?' + urllib.parse.urlencode({'path':node}), token=self.audit_token)
        if 'Sys.Audit' not in permissions.get(node, {}):
            raise Rejected('incomplete_task_history', 'Read-only node Sys.Audit is required to see every task owner.')
        return self.call(node + '/tasks?' + query, token=self.audit_token)

    def verify_clean(self, rollback):
        if not rollback or not self.matches_task(rollback, 'resetting'):
            raise Rejected('slot_reconciliation_required', 'Attest the clean rollback of the stopped slot.')
        # Older database backups must never re-admit a slot used since this rollback.
        for kind in ('qmrollback', 'qmstart', 'qmresume'):
            query = urllib.parse.urlencode({'source':'all','vmid':self.guest_id,'typefilter':kind,'limit':1})
            tasks = self.audit_tasks(query)
            if kind == 'qmrollback':
                if not tasks or tasks[0]['upid'] != rollback or tasks[0].get('status') != 'OK':
                    raise Rejected('slot_reconciliation_required', 'Clean rollback does not match provider history.')
            elif tasks and tasks[0]['starttime'] >= int(rollback.split(':')[4], 16):
                raise Rejected('slot_reconciliation_required', 'Guest ran since its attested rollback.')

    def status(self):
        self.verified_config()
        return self.call(self.guest + '/status/current')['status']

    def start(self):
        self.verified_config()
        return self.call(self.guest + '/status/start', {})

    def shutdown(self):
        self.verified_config()
        return self.call(self.guest + '/status/shutdown', {'timeout': 20})

    def reset(self):
        self.verified_config()
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
        tasks = self.audit_tasks(query)
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

    def install_guest_key(self, key, lease):
        self.verified_config()
        self.call(self.guest + '/agent/file-write', {
            'file': '/etc/ssh/lease_authorized_keys',
            'content': f'{key} lease-{lease}\n',
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

    def read_observation(self, *, require_slot=True, allow_running_slot=False):
        try:
            return self.observe(require_slot=require_slot, allow_running_slot=allow_running_slot)
        except (OSError, ValueError, TypeError, KeyError) as error:
            print(f'Capacity observation failed: {type(error).__name__}', flush=True)
            raise Rejected('capacity_unverified', 'Fresh host capacity could not be verified. Try again later.', 503) from error

    def check(self, reservations=(0, 0), *, require_slot=True, allow_running_slot=False):
        observation = self.read_observation(require_slot=require_slot, allow_running_slot=allow_running_slot)
        for value, minimum, code, label in (
            (observation['memory_after_start'] - reservations[0] * MIB, self.memory_floor, 'host_memory_headroom', 'memory'),
            (observation['storage_after_writes'] - reservations[1] * MIB, self.storage_floor, 'host_storage_headroom', 'storage'),
        ):
            if value < minimum:
                raise Rejected(code, f'This request would leave {max(0, value) // MIB} MiB of host {label} headroom; {minimum // MIB} MiB is required.')

        return observation

    def observe(self, *, require_slot=True, allow_running_slot=False):
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
        if (slot['name'] != self.provider.name or slot.get('template')
                or (slot['status'] != 'stopped' and not allow_running_slot)):
            raise ValueError('The capacity view does not match the stopped pilot guest')
        reserved = sum(self.positive(row['maxmem']) for row in guests.values()
                       if row['status'] == 'running' or str(row['vmid']) in self.protected_ids)
        total, available = self.positive(memory['total']), self.positive(memory['available'])
        if available > total or not storage['active'] or not storage['enabled']:
            raise ValueError('Invalid node memory or inactive slot storage')
        available_storage = self.positive(storage['avail'])
        pending_slot_memory = self.positive(slot['maxmem']) if require_slot and slot['status'] == 'stopped' else 0
        slot_disk_reserve = self.positive(slot['maxdisk']) if require_slot else 0
        return {
            'slot_resources': {'memory_mib': self.positive(slot['maxmem']) // MIB,
                               'disk_mib': self.positive(slot['maxdisk']) // MIB,
                               'vcpus': self.positive(slot['maxcpu'])},
            'memory_after_start': min(available, total - reserved)
            - self.reserved_memory - pending_slot_memory,
            'storage_after_writes': available_storage - slot_disk_reserve,
        }

    @staticmethod
    def positive(value):
        if type(value) is not int or value <= 0:
            raise ValueError('Expected a positive byte count')
        return value


class Pilot:
    def __init__(self, config, *, enroll=False):
        self.provider = Proxmox(config['proxmox'])
        self.capacity = Capacity(self.provider, config['capacity'])
        self.provider.audit_token = self.capacity.token
        self.db = sqlite3.connect(config['database'], factory=Ledger)
        self.db.row_factory = sqlite3.Row
        self.db.execute('PRAGMA journal_mode=WAL')
        self.db.execute('PRAGMA synchronous=FULL')
        self.db.execute('PRAGMA busy_timeout=5000')
        if config['database'] != ':memory:':
            controller_id = int(config['controller_guest_id'])
            def controller_generation():
                vm = self.provider.call(f'/nodes/{self.provider.node}/qemu/{controller_id}/config',
                                        token=self.capacity.token)
                return str(uuid.UUID(vm['vmgenid']))
            if enroll:
                if self.provider.status() != 'stopped':
                    raise ValueError('Enrollment requires an idle verified slot.')
                existing = self.db.execute("SELECT 1 FROM sqlite_master WHERE name='lease'").fetchone()
                if existing and self.db.execute("SELECT 1 FROM lease WHERE phase!='released'").fetchone():
                    raise ValueError('Reconcile active lease ownership before enrollment.')
                self.provider.verify_clean(self.provider.initial_rollback)
            self.db.attach(config['ledger_witness'], controller_generation, enroll=enroll)
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
                ('generation', 'TEXT'), ('clean_rollback', 'TEXT'), ('restore_completed_at', 'REAL'),
                ('resume_point', 'TEXT'),
                ('requirements', "TEXT NOT NULL DEFAULT '{}'"),
                ('prior_tasks', 'TEXT'),
                ('owner', "TEXT NOT NULL DEFAULT 'operator'"),
                ('ssh_key', 'TEXT'),
                ('reclaim_on_expiry', 'INTEGER NOT NULL DEFAULT 0'),
                ('reviewer', 'TEXT'),
                ('reviewer_hold', 'TEXT'),
                ('reviewer_reason', 'TEXT'),
                ('reviewer_override', 'TEXT'),
                ('hold_mode', 'TEXT'),
                ('hold_reason', 'TEXT'),
                ('export_challenge', 'TEXT'), ('storage_challenge', 'TEXT'),
                ('export_start_tasks', 'TEXT'),
                ('export_requested_at', 'REAL'),
                ('export_failed_at', 'REAL'),
                ('export_error', 'TEXT'),
                ('export_disposition', 'TEXT'),
                ('checkpoint', 'TEXT'),
                ('released_at', 'REAL'),
            ):
                if column not in columns:
                    self.db.execute(f'ALTER TABLE lease ADD COLUMN {column} {definition}')
        with self.db:
            self.db.execute('''CREATE TABLE IF NOT EXISTS recovery_ticket (
                token_hash TEXT PRIMARY KEY, lease_id TEXT NOT NULL,
                expires_at REAL NOT NULL, redeemed INTEGER NOT NULL DEFAULT 0
            )''')
            self.db.execute('''CREATE TABLE IF NOT EXISTS renewal_delegate (
                lease_id TEXT PRIMARY KEY, token_hash TEXT NOT NULL,
                valid_until REAL NOT NULL, extension INTEGER NOT NULL
            )''')
            self.db.execute("UPDATE lease SET storage_challenge=export_challenge WHERE storage_challenge IS NULL AND export_challenge IS NOT NULL")
            self.db.execute("""CREATE TABLE IF NOT EXISTS restore_hold (
                replacement TEXT PRIMARY KEY, source TEXT NOT NULL, checkpoint_id TEXT)""")
            self.db.execute("""INSERT OR IGNORE INTO restore_hold
                SELECT id, json_extract(resume_point,'$.lease'), json_extract(resume_point,'$.checkpoint_id')
                FROM lease WHERE phase!='released' AND resume_point IS NOT NULL AND restore_completed_at IS NULL""")
            self.db.execute("""CREATE TABLE IF NOT EXISTS export_alert (
                id TEXT PRIMARY KEY, challenge TEXT NOT NULL UNIQUE,
                lease_id TEXT NOT NULL, owner TEXT NOT NULL,
                failed_at REAL NOT NULL, delivered_at REAL)""")
            self.db.execute("""CREATE TABLE IF NOT EXISTS unacknowledged_expiry (
                challenge TEXT PRIMARY KEY, claim TEXT NOT NULL, purged_at REAL)""")
        ticket_columns = {row['name'] for row in self.db.execute('PRAGMA table_info(recovery_ticket)')}
        if 'checkpoint_id' not in ticket_columns:
            with self.db:
                self.db.execute('ALTER TABLE recovery_ticket ADD COLUMN checkpoint_id TEXT')
        self.checkpoints = Checkpoints(self.db)
        self.static_principals = frozenset(config['client_tokens'])
        self.task_credentials = TaskCredentials(self.db, config.get('task_credential_max_seconds', 86400))
        recovery = config.get('recovery', {})
        self.recovery_endpoint = recovery.get('endpoint')
        self.upload_endpoint = self.recovery_endpoint if recovery.get('accept_uploads') is True else None
        if bool(self.recovery_endpoint) != bool(recovery.get('worker_token_file')):
            raise ValueError('Recovery needs an HTTPS endpoint and a dedicated worker credential')
        self.reservations = Reservations(self.db, self.check_reservation_capacity)
        self.user = config['guest_user']
        self.host_key = public_key(config['guest_host_key'])
        policy = config.get('reclamation', {})
        self.reclaim_enabled = bool(policy.get('verifier_token_file'))
        self.alerts_enabled = bool(policy.get('alert_token_file'))
        self.retention_enabled = policy.get('purge_expired_checkpoints') is True
        if self.retention_enabled and not self.reclaim_enabled:
            raise ValueError('Checkpoint expiry requires a verifier')
        self.grace_seconds = policy.get('grace_seconds', 7200)
        if type(self.grace_seconds) is not int or self.grace_seconds < 1:
            raise ValueError('Grace must be a positive duration')

    def check_reservation_capacity(self, reservations):
        active = self.db.execute("SELECT 1 FROM lease WHERE phase!='released'").fetchone()
        self.capacity.check(reservations, require_slot=bool(active), allow_running_slot=bool(active))

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

    def verify_generation(self, row):
        self.db.verify()
        if not row['generation'] or self.provider.generation() != row['generation']:
            raise Rejected('slot_reconciliation_required', 'Provider generation differs from the recorded lease. Hold for operator reconciliation.')

    def dispatch(self, lease, phase, operation, *, recorded=False):
        self.verify_generation(self.get(lease))
        if not recorded:
            prior = json.dumps(sorted(self.provider.recent_tasks(phase)))
            self.update(lease, phase, prior_tasks=prior)
        # Dispatch once. Recovery can only attach a newly observed, matching
        # provider task to this intent. With zero or multiple matches, hold.
        self.verify_generation(self.get(lease))
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
        if phase != 'released' and phase != 'resetting':
            self.verify_generation(row)
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
            self.provider.install_guest_key(row['ssh_key'], lease)
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
            self.provider.verify_clean(row['upstream'])
            if self.provider.generation() == row['generation']:
                raise Rejected('slot_reconciliation_required', 'Rollback has not advanced the slot generation.')
            released_at = time.time()
            retention = released_at + CHECKPOINT_RETENTION_SECONDS
            receipt = json.loads(row['checkpoint']) if row['checkpoint'] else None
            if receipt:
                receipt['retain_through'] = retention
            with self.db:
                self.db.execute("""UPDATE lease SET phase='released', clean_rollback=upstream, upstream=NULL, address=NULL,
                    prior_tasks=NULL, checkpoint=?, released_at=? WHERE id=?""",
                                (json.dumps(receipt) if receipt else None, released_at, lease))
                self.db.execute("""UPDATE incremental_checkpoint
                    SET receipt=json_set(receipt, '$.retain_through', ?)
                    WHERE lease_id=? AND receipt IS NOT NULL""", (retention, lease))
                self.db.execute('DELETE FROM restore_hold WHERE replacement=?', (lease,))
            row = self.get(lease)
        elif phase == 'ready':
            power = self.provider.status()
            address = self.provider.ip() if power == 'running' else None
            if address != row['address']:
                row = self.update(lease, 'ready', address=address)
            if not address:
                return self.public(row, 'waiting-for-guest-ip' if power == 'running' else 'guest-stopped')
        status = row['phase']
        if status == 'ready' and time.time() >= row['deadline']:
            status = 'overdue'
        return self.public(row, status)

    def public(self, row, status=None):
        result = {key: value for key, value in row.items() if key not in ('prior_tasks', 'ssh_key', 'request_key', 'export_challenge', 'storage_challenge', 'export_start_tasks', 'checkpoint')}
        result['requirements'] = json.loads(row['requirements'])
        result['resume_point'] = json.loads(row['resume_point']) if row['resume_point'] else None
        result['status'] = status or row['phase']
        if incremental := self.checkpoints.list(row['id']):
            result['incremental_checkpoints'] = incremental
        if row['checkpoint']:
            checkpoint = json.loads(row['checkpoint'])
            checkpoint.pop('purge_claim', None)
            result['checkpoint'] = checkpoint
        if row['export_disposition']:
            result['export_disposition'] = json.loads(row['export_disposition'])
        if row['export_failed_at']:
            result['export_needs_decision'] = (not row['export_disposition']
                and time.time() >= row['export_failed_at'] + EXPORT_REVIEW_SECONDS)
        if row['address']:
            result['ssh'] = {'host': row['address'], 'user': self.user,
                             'host_key_alias': self.provider.name, 'host_key': self.host_key}
        return result

    def matching_point(self, source, owner, requested_id, replacement_key):
        if not isinstance(source, str) or not re.fullmatch('[a-f0-9]{32}', source):
            raise Rejected('invalid_source', 'Provide the predecessor lease ID.', 400)
        previous = self.owned(source, owner)
        if previous['ssh_key'] == replacement_key:
            raise Rejected('new_ssh_key_required', 'Generate a fresh SSH key for the replacement.', 400)
        if previous['phase'] != 'released':
            raise Rejected('source_active', 'Reconnect to the existing lease until reclamation finishes.')
        now = time.time()
        candidates = [requested_id]
        if requested_id is None:
            candidates.extend(r['id'] for r in self.db.execute(
                'SELECT id FROM incremental_checkpoint WHERE lease_id=? AND receipt IS NOT NULL ORDER BY created_at DESC',
                (source,)))
        for candidate in candidates:
            try:
                receipt = self.available_checkpoint(previous, now, candidate)
            except Rejected as error:
                if error.code != 'checkpoint_unavailable':
                    raise
                continue
            return {'lease': source, 'requested_checkpoint': requested_id, 'checkpoint_id': candidate,
                    'sha256': receipt['sha256'], 'verified_at': receipt['verified_at'],
                    'scope': 'declared workspace', 'capture': 'incremental' if candidate else 'final',
                    'requirements': json.loads(previous['requirements'])}
        raise Rejected('checkpoint_unavailable', 'No acknowledged recoverable version remains.', 404)

    def acquire(self, data, owner):
        task, key, seconds = data.get('task'), data.get('request_key'), data.get('seconds')
        if not all(isinstance(s, str) and 0 < len(s) <= 128 for s in (task, key)):
            raise Rejected('invalid_request', 'Provide a task and a request_key of 1–128 characters.', 400)
        self.check_duration(seconds)
        reclaim = data.get('reclaim_on_expiry', True)
        if type(reclaim) is not bool or (reclaim and not self.reclaim_enabled):
            raise Rejected('reclamation_unavailable', 'Automatic reclamation is unavailable for this request.', 422)
        key = owner + '/' + key
        ssh_key = public_key(data.get('ssh_public_key'))
        requested = data.get('requirements', {})
        encoded = requirements(requested)
        if data.get('recipe', RECIPE) != RECIPE:
            raise Rejected('unsupported_recipe', 'The pilot provides only debian13-amd64.', 422)
        prev = self.db.execute('SELECT * FROM lease WHERE request_key=?', (key,)).fetchone()
        resume_from = data.get('resume_from')
        requested_checkpoint = data.get('checkpoint_id')
        if requested_checkpoint is not None and not resume_from:
            raise Rejected('invalid_source', 'Select a predecessor lease for this checkpoint.', 400)
        point = None
        if prev:
            point = json.loads(prev['resume_point'] or '{}')
            if point.get('lease') != resume_from or point.get('requested_checkpoint') != requested_checkpoint:
                raise Rejected('request_key_conflict', 'This request key identifies another recovery source.')
            if resume_from and 'requirements' not in data:
                encoded = requirements(point['requirements'])
            if prev['requirements'] != encoded or prev['task'] != task or prev['initial_seconds'] != seconds or prev['ssh_key'] != ssh_key or ('reclaim_on_expiry' in data and bool(prev['reclaim_on_expiry']) != reclaim):
                raise Rejected('request_key_conflict', 'This request key is already bound to different inputs.')
            return self.inspect(prev['id'], owner)
        if not reclaim:
            raise Rejected('reclamation_required', 'Every new lease uses checkpoint-gated reclamation.', 422)
        if resume_from is not None:
            point = self.matching_point(resume_from, owner, requested_checkpoint, ssh_key)
            if 'requirements' in data and requested != point['requirements']:
                raise Rejected('not_matching', 'A matching replacement uses the predecessor requirements.', 422)
            requested = point['requirements']
            encoded = requirements(requested)
        if self.db.execute("SELECT 1 FROM lease WHERE phase!='released'").fetchone():
            raise Rejected('capacity_exhausted', 'The single pilot slot is already reserved.')
        if self.provider.status() != 'stopped':
            raise Rejected('slot_unavailable', 'The pilot guest is already running outside the controller.')
        last = self.db.execute("SELECT clean_rollback FROM lease WHERE clean_rollback IS NOT NULL ORDER BY rowid DESC LIMIT 1").fetchone()
        self.provider.verify_clean(last[0] if last else self.provider.initial_rollback)
        generation = self.provider.generation()
        prior = json.dumps(sorted(self.provider.recent_tasks('starting')))
        observed = self.capacity.check(self.reservations.totals())
        match(requested, observed['slot_resources'])
        lease = uuid.uuid4().hex
        with self.db:
            self.db.execute('''INSERT INTO lease
                (id, request_key, task, initial_seconds, phase, deadline, prior_tasks, owner, ssh_key, reclaim_on_expiry, requirements, resume_point, generation)
                VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)''',
                            (lease, key, task, seconds, 'starting', time.time()+seconds, prior, owner, ssh_key, int(reclaim), encoded, json.dumps(point) if point else None, generation))
            if point:
                self.db.execute('INSERT INTO restore_hold VALUES (?,?,?)', (lease, point['lease'], point['checkpoint_id']))
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

    def delegate_renewal(self, lease, data, owner):
        row = self.owned(lease, owner)
        if data.get('revoke') is True:
            with self.db:
                self.db.execute('DELETE FROM renewal_delegate WHERE lease_id=?', (lease,))
            return {'revoked': True}
        if row['phase'] not in ('starting', 'ready'):
            raise Rejected('lease_not_renewable', 'Only a live or starting lease can delegate renewal.')
        duration, extension = data.get('duration_seconds'), data.get('extension_seconds')
        self.check_duration(duration)
        self.check_duration(extension)
        if extension >= duration:
            raise Rejected('invalid_delegation', 'The extension must be shorter than the delegation window.', 400)
        token = secrets.token_urlsafe(32)
        token_hash = hashlib.sha256(token.encode('ascii')).hexdigest()
        valid_until = time.time() + duration
        with self.db:
            self.db.execute("""INSERT INTO renewal_delegate VALUES (?,?,?,?)
                ON CONFLICT(lease_id) DO UPDATE SET token_hash=excluded.token_hash,
                valid_until=excluded.valid_until, extension=excluded.extension""",
                            (lease, token_hash, valid_until, extension))
        return {'token': token, 'valid_until': valid_until, 'extension_seconds': extension}

    def delegated_renew(self, lease, token):
        if not re.fullmatch(r'[A-Za-z0-9_-]{43}', token):
            raise Rejected('delegation_unavailable', 'No valid renewal delegation.', 401)
        token_hash = hashlib.sha256(token.encode('ascii')).hexdigest()
        delegation = self.db.execute('SELECT * FROM renewal_delegate WHERE lease_id=?',
                                     (lease,)).fetchone()
        now = time.time()
        if (not delegation or delegation['valid_until'] <= now
                or not hmac.compare_digest(token_hash, delegation['token_hash'])):
            raise Rejected('delegation_unavailable', 'No valid renewal delegation.', 401)
        row = self.get(lease)
        if row['phase'] not in ('starting', 'ready'):
            raise Rejected('lease_not_renewable', 'Reclamation has already begun.')
        deadline = max(row['deadline'], min(now + delegation['extension'], delegation['valid_until']))
        with self.db:
            self.db.execute('UPDATE lease SET deadline=? WHERE id=?', (deadline, lease))
        return {'deadline': deadline, 'delegated_until': delegation['valid_until']}

    def assign_reviewer(self, lease, data, owner):
        row = self.owned(lease, owner)
        if row['phase'] not in ('starting','ready','awaiting-export') or not row['reclaim_on_expiry']:
            raise Rejected('hold_unavailable', 'Only an enrolled lease before reset accepts review.')
        reviewer = data.get('reviewer')
        if reviewer is not None:
            if not isinstance(reviewer, str) or not reviewer or reviewer == owner:
                raise Rejected('invalid_reviewer', 'Choose a separate existing task principal.', 400)
            issued = self.db.execute("""SELECT 1 FROM task_credential
                WHERE owner=? AND revoked_at IS NULL AND expires_at>?""", (reviewer, time.time())).fetchone()
            if reviewer not in self.static_principals and not issued:
                raise Rejected('invalid_reviewer', 'Choose an active task principal.', 400)
        if row['reviewer_hold'] and reviewer != row['reviewer']:
            raise Rejected('lease_held', 'Resolve the designated reviewer hold before changing reviewers.')
        with self.db:
            self.db.execute('UPDATE lease SET reviewer=? WHERE id=?', (reviewer, lease))
        return self.public(self.get(lease))

    def override_review_hold(self, lease, data, owner):
        row = self.owned(lease, owner)
        reason = data.get('reason')
        if (not row['reviewer_hold'] or data.get('confirm_reviewer') != row['reviewer']
                or not isinstance(reason, str) or not 1 <= len(reason.strip()) <= 240):
            raise Rejected('review_confirmation_required', 'Confirm the affected reviewer and give a reason.')
        decision = {'at': time.time(), 'owner': owner, 'reviewer': row['reviewer'],
                    'hold': row['reviewer_hold'], 'prior_reason': row['reviewer_reason'],
                    'reason': reason.strip(), 'disposition': 'Remove reviewer hold; standard checkpoint policy applies.'}
        with self.db:
            self.db.execute("""UPDATE lease SET reviewer=NULL, reviewer_hold=NULL, reviewer_reason=NULL,
                reviewer_override=? WHERE id=?""", (json.dumps(decision), lease))
        return self.public(self.get(lease))

    def hold(self, lease, data, principal, *, delegated=False):
        row = self.get(lease)
        if principal != row['reviewer' if delegated else 'owner']:
            raise Rejected('unknown_lease', 'No lease has this identifier.', 404)
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
            fields = 'reviewer_hold=?, reviewer_reason=?' if delegated else 'hold_mode=?, hold_reason=?'
            self.db.execute(f'UPDATE lease SET {fields} WHERE id=?',
                            (None if mode == 'clear' else mode,
                             None if mode == 'clear' else reason.strip(), lease))
        if delegated:
            return {'id': lease, 'hold': mode, 'reviewer': principal}
        return self.public(self.get(lease))

    def request_export(self, lease):
        self.verify_generation(self.get(lease))
        start_tasks = self.provider.slot_start_tasks()
        challenge = uuid.uuid4().hex
        with self.db:
            self.db.execute("""UPDATE lease SET phase='awaiting-export', upstream=NULL,
                prior_tasks=NULL, address=NULL, export_challenge=?, storage_challenge=?, export_requested_at=?,
                export_start_tasks=? WHERE id=?""", (challenge, challenge, time.time(), start_tasks, lease))
        return self.get(lease)

    def retry_export(self, lease, owner):
        row = self.owned(lease, owner)
        if row['phase'] != 'awaiting-export' or row['checkpoint'] or not row['export_failed_at']:
            raise Rejected('export_retry_unavailable', 'Only a failed pending export can be retried.')
        self.check_export_source(row)
        with self.db:
            self.db.execute(
                'UPDATE lease SET export_challenge=?, export_failed_at=NULL, export_error=NULL,'
                ' export_disposition=NULL WHERE id=?',
                (uuid.uuid4().hex, lease))
        return self.public(self.get(lease))

    def decide_failed_export(self, lease, data, owner):
        row = self.owned(lease, owner)
        mode, reason = data.get('mode'), data.get('reason')
        if (mode not in ('retain', 'discard') or not isinstance(reason, str)
                or not 1 <= len(reason.strip()) <= 240):
            raise Rejected('invalid_disposition', 'Choose retain or discard and give a short reason.', 400)
        if mode == 'discard' and data.get('confirm_lease') != lease:
            raise Rejected('discard_confirmation_required', 'Explicitly confirm the exact lease ID.', 400)
        previous = json.loads(row['export_disposition'] or '{}')
        if previous.get('mode') == 'discard':
            return self.inspect(lease, owner)
        if (row['phase'] != 'awaiting-export' or row['checkpoint'] or not row['export_failed_at']
                or time.time() < row['export_failed_at'] + EXPORT_REVIEW_SECONDS):
            raise Rejected('disposition_unavailable', 'This lease has no unresolved 24-hour export failure.')
        decision = json.dumps({'mode': mode, 'reason': reason.strip(), 'at': time.time()})
        if mode == 'retain':
            with self.db:
                self.db.execute("""UPDATE lease SET hold_mode='disk', hold_reason=?,
                    export_disposition=? WHERE id=?""", (reason.strip(), decision, lease))
            return self.public(self.get(lease))

        if row['reviewer_hold']:
            raise Rejected('lease_held', 'Resolve the designated reviewer hold before discarding this disk.')
        self.check_export_source(row)
        prior = json.dumps(sorted(self.provider.recent_tasks('resetting')))
        # Persist the explicit data disposition and provider intent together.
        with self.db:
            self.db.execute("""UPDATE lease SET phase='resetting', upstream=NULL, prior_tasks=?,
                export_disposition=?, hold_mode=NULL, hold_reason=NULL WHERE id=?""",
                            (prior, decision, lease))
        return self.public(self.dispatch(lease, 'resetting', self.provider.reset, recorded=True))

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
        return {'lease_id': lease, 'challenge': row['export_challenge'], 'storage_challenge': row['storage_challenge'],
                'guest_id': self.provider.guest_id, 'requested_at': row['export_requested_at']}

    def check_export_source(self, row):
        self.verify_generation(row)
        if (self.provider.status() != 'stopped' or
                self.provider.slot_start_tasks() != row['export_start_tasks']):
            raise Rejected('export_source_changed', 'The guest start history or power state changed. Preserve its disk for review.')

    def export_result(self, lease, data):
        row = self.get(lease)
        if row['checkpoint']:
            receipt = json.loads(row['checkpoint'])
            if (data.get('outcome') == 'verified' and data.get('challenge') == row['export_challenge']
                    and all(data.get(field) == receipt.get(field) for field in ('snapshot', 'path', 'sha256', 'storage_managed'))):
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
                if self.alerts_enabled:
                    self.db.execute("""INSERT OR IGNORE INTO export_alert
                        SELECT ?, export_challenge, id, owner, export_failed_at, NULL
                        FROM lease WHERE id=?""", (uuid.uuid4().hex, lease))
            print(f'Final export failed for lease {lease}; disk preserved', flush=True)
            return self.public(self.get(lease))
        snapshot, digest, path = data.get('snapshot'), data.get('sha256'), data.get('path')
        if (data.get('outcome') != 'verified' or not isinstance(snapshot, str)
                or not re.fullmatch(r'[a-f0-9]{64}', snapshot)
                or path != '/lease-' + lease + '.tar.gz'
                or not isinstance(digest, str) or not re.fullmatch(r'[a-f0-9]{64}', digest)):
            raise Rejected('invalid_export_receipt', 'Provide a verified encrypted snapshot and download digest.', 400)
        managed = data.get('storage_managed')
        if managed is not True or data.get('restore_verified') is not True:
            raise Rejected('invalid_export_receipt', 'Final export requires managed storage and verified workspace restore.', 400)
        receipt = {'snapshot': snapshot, 'path': path, 'sha256': digest,
                   'verified_at': time.time(), 'storage_managed': managed, 'restore_verified': True}
        with self.db:
            self.db.execute('UPDATE lease SET checkpoint=?, export_failed_at=NULL, export_error=NULL WHERE id=?',
                            (json.dumps(receipt), lease))
        return self.public(self.get(lease))

    def next_export_alert(self):
        row = self.db.execute("""SELECT id, lease_id, owner, failed_at
                                FROM export_alert WHERE delivered_at IS NULL
                                ORDER BY failed_at, id LIMIT 1""").fetchone()
        return {'alert': dict(row) if row else None}

    def acknowledge_export_alert(self, data):
        alert_id = data.get('alert_id')
        if not isinstance(alert_id, str) or not re.fullmatch(r'[a-f0-9]{32}', alert_id):
            raise Rejected('invalid_alert', 'Provide a valid alert ID.', 400)
        with self.db:
            result = self.db.execute("""UPDATE export_alert
                      SET delivered_at=coalesce(delivered_at, ?) WHERE id=?""",
                                     (time.time(), alert_id))
            if result.rowcount != 1:
                raise Rejected('unknown_alert', 'No matching export alert.', 404)
        return {'delivered': True}

    def grant_upload(self, lease, data, owner):
        row = self.owned(lease, owner)
        # A completed request remains inspectable by request key after release.
        endpoint = self.upload_endpoint if row['phase'] == 'ready' and row['reclaim_on_expiry'] else None
        return self.checkpoints.grant(lease, data, endpoint)

    def redeem_upload(self, data):
        if not self.upload_endpoint:
            raise Rejected('checkpoint_unavailable', 'Incremental uploads are unavailable.', 503)
        row = self.checkpoints.ticket(data.get('ticket'), redeemed=0)
        if self.get(row['lease_id'])['phase'] != 'ready':
            raise Rejected('checkpoint_unavailable', 'The lease is no longer ready for uploads.')
        return self.checkpoints.redeem(data['ticket'])

    def complete_upload(self, data):
        row = self.checkpoints.ticket(data.get('ticket'), redeemed=1)
        released_at = self.get(row['lease_id'])['released_at']
        return self.checkpoints.complete(data, released_at + CHECKPOINT_RETENTION_SECONDS if released_at else None)

    def checkpoint(self, row, checkpoint_id):
        if checkpoint_id is not None:
            if not isinstance(checkpoint_id, str) or not re.fullmatch(r'[a-f0-9]{32}', checkpoint_id):
                raise Rejected('invalid_checkpoint', 'A checkpoint ID must be 32 lowercase hex characters.', 400)
            return json.loads(self.checkpoints.get(row['id'], checkpoint_id)['receipt'] or '{}')
        return json.loads(row['checkpoint'] or '{}')

    def set_pin(self, lease, data, owner):
        row = self.owned(lease, owner)
        checkpoint_id = data.get('checkpoint_id')
        checkpoint = self.checkpoint(row, checkpoint_id)
        if not checkpoint.get('storage_managed') or (checkpoint_id is None and row['phase'] != 'released'):
            raise Rejected('checkpoint_not_managed', 'Only an available managed checkpoint can be pinned.')
        if checkpoint.get('purge_claim') or checkpoint.get('purged_at'):
            raise Rejected('checkpoint_expiring', 'This checkpoint has already entered expiry.')
        pinned, reason = data.get('pinned'), data.get('reason')
        if type(pinned) is not bool or (pinned and (
                not isinstance(reason, str) or not 1 <= len(reason.strip()) <= 240)):
            raise Rejected('invalid_pin', 'Choose pin with a short reason, or unpin.', 400)
        if pinned:
            checkpoint['pin'] = {'reason': reason.strip(), 'at': time.time()}
        else:
            checkpoint.pop('pin', None)
        self.save_checkpoint(lease, checkpoint, checkpoint_id)
        return self.public(self.get(lease))

    def save_checkpoint(self, lease, checkpoint, checkpoint_id=None):
        if checkpoint_id is not None:
            self.checkpoints.save(checkpoint_id, checkpoint)
            return
        with self.db:
            self.db.execute('UPDATE lease SET checkpoint=? WHERE id=?',
                            (json.dumps(checkpoint), lease))

    def claim_expired_checkpoint(self):
        if not self.retention_enabled:
            return {'expiry': None}
        row = self.db.execute("""WITH artifacts AS (
            SELECT id AS lease_id, NULL AS checkpoint_id, storage_challenge AS challenge, checkpoint AS receipt
                FROM lease WHERE phase='released'
            UNION ALL
            SELECT lease_id, id AS checkpoint_id, id AS challenge, receipt FROM incremental_checkpoint
        )
        SELECT * FROM artifacts
        WHERE json_extract(receipt, '$.storage_managed')=1
            AND json_extract(receipt, '$.pin') IS NULL
            AND json_extract(receipt, '$.purged_at') IS NULL
            AND json_extract(receipt, '$.retain_through') <= ?
            AND NOT EXISTS (SELECT 1 FROM restore_hold h WHERE h.source=artifacts.lease_id AND h.checkpoint_id IS artifacts.checkpoint_id)
            AND NOT EXISTS (SELECT 1 FROM recovery_ticket
                            WHERE lease_id=artifacts.lease_id AND expires_at > ?)
        ORDER BY json_extract(receipt, '$.retain_through'), lease_id, checkpoint_id LIMIT 1""",
                              (time.time(), time.time())).fetchone()
        if row is None:
            # A write may have reached storage just before readback or acknowledgment failed.
            orphan = self.db.execute("""WITH attempts AS (
                SELECT id AS source, NULL AS checkpoint_id, storage_challenge AS challenge, released_at
                    FROM lease WHERE phase='released' AND checkpoint IS NULL
                UNION ALL
                SELECT l.id, i.id, i.id, l.released_at FROM incremental_checkpoint i
                    JOIN lease l ON l.id=i.lease_id WHERE l.phase='released' AND i.receipt IS NULL)
                SELECT a.*, e.claim FROM attempts a LEFT JOIN unacknowledged_expiry e USING(challenge)
                WHERE a.challenge IS NOT NULL AND e.purged_at IS NULL AND a.released_at<=?
                ORDER BY a.released_at LIMIT 1""", (time.time()-CHECKPOINT_RETENTION_SECONDS,)).fetchone()
            if orphan is None:
                return {'expiry': None}
            claim = orphan['claim'] or uuid.uuid4().hex
            with self.db:
                self.db.execute('INSERT OR IGNORE INTO unacknowledged_expiry VALUES (?,?,NULL)',
                                (orphan['challenge'], claim))
            return {'expiry': {'lease_id': orphan['source'], 'checkpoint_id': orphan['checkpoint_id'],
                               'challenge': orphan['challenge'], 'claim': claim, 'snapshot': None,
                               'unacknowledged': True, 'path': '/' + object_name(orphan['source'], orphan['checkpoint_id'])}}
        lease, checkpoint_id = row['lease_id'], row['checkpoint_id']
        checkpoint = json.loads(row['receipt'])
        if not checkpoint.get('purge_claim'):
            checkpoint['purge_claim'] = uuid.uuid4().hex
            checkpoint['purge_started_at'] = time.time()
            self.save_checkpoint(lease, checkpoint, checkpoint_id)
        return {'expiry': {'lease_id': lease, 'challenge': row['challenge'],
                           'checkpoint_id': checkpoint_id, 'claim': checkpoint['purge_claim'],
                           'snapshot': checkpoint['snapshot'], 'path': checkpoint['path']}}

    def confirm_checkpoint_expiry(self, data):
        row = self.get(data.get('lease_id'))
        checkpoint_id = data.get('checkpoint_id')
        if data.get('unacknowledged') is True:
            challenge = checkpoint_id or row['storage_challenge']
            if checkpoint_id is not None:
                self.checkpoints.get(row['id'], checkpoint_id)
            if not self.retention_enabled or row['phase'] != 'released' or data.get('absent') is not True:
                raise Rejected('expiry_receipt_mismatch', 'Invalid unacknowledged expiry result.')
            with self.db:
                changed = self.db.execute('UPDATE unacknowledged_expiry SET purged_at=coalesce(purged_at,?) WHERE challenge=? AND claim=?',
                                          (time.time(), challenge, data.get('claim')))
                if changed.rowcount != 1:
                    raise Rejected('expiry_receipt_mismatch', 'Unknown expiry claim.')
            return {'absent': True}
        checkpoint = self.checkpoint(row, checkpoint_id)
        if (not self.retention_enabled or not checkpoint.get('purge_claim')
                or data.get('claim') != checkpoint['purge_claim']
                or data.get('snapshot') != checkpoint['snapshot']
                or data.get('absent') is not True):
            raise Rejected('expiry_receipt_mismatch', 'No matching expiry claim and verified removal.')
        if not checkpoint.get('purged_at'):
            checkpoint['purged_at'] = time.time()
            self.save_checkpoint(row['id'], checkpoint, checkpoint_id)
        return self.public(self.get(row['id']))

    def available_checkpoint(self, row, now, checkpoint_id, *, ticket_held=False):
        checkpoint = self.checkpoint(row, checkpoint_id)
        if not recoverable(checkpoint, row['phase'], checkpoint_id is None, now,
                           ticket_held or restore_held(self.db, row['id'], checkpoint_id)):
            raise Rejected('checkpoint_unavailable', 'No retained checkpoint is available for this lease.', 404)
        return checkpoint

    def finish_restore(self, lease, owner, data):
        row = self.owned(lease, owner)
        point = json.loads(row['resume_point'] or 'null')
        if not point or point['sha256'] != data.get('sha256') or row['phase'] != 'ready':
            raise Rejected('restore_mismatch', 'Restore acknowledgment must match the ready replacement.')
        with self.db:
            self.db.execute('UPDATE lease SET restore_completed_at=coalesce(restore_completed_at,?) WHERE id=?', (time.time(), lease))
            self.db.execute('DELETE FROM restore_hold WHERE replacement=?', (lease,))
        return {'restored': True}

    def recovery_ticket(self, lease, owner, data):
        if not self.recovery_endpoint:
            raise Rejected('recovery_unavailable', 'Checkpoint recovery is unavailable.', 503)
        row = self.owned(lease, owner)
        now = time.time()
        checkpoint_id = data.get('checkpoint_id')
        checkpoint = self.available_checkpoint(row, now, checkpoint_id)
        token = secrets.token_urlsafe(32)
        token_hash = hashlib.sha256(token.encode('ascii')).hexdigest()
        expiry = now + RECOVERY_TICKET_SECONDS
        with self.db:
            self.db.execute('DELETE FROM recovery_ticket WHERE expires_at <= ?', (now,))
            self.db.execute('DELETE FROM recovery_ticket WHERE lease_id=? AND redeemed=0', (lease,))
            self.db.execute('''INSERT INTO recovery_ticket (token_hash, lease_id, expires_at, checkpoint_id)
                            VALUES (?,?,?,?)''',
                            (token_hash, lease, expiry, checkpoint_id))
        return {'endpoint': self.recovery_endpoint, 'ticket': token, 'expires_at': expiry,
                'sha256': checkpoint['sha256']}

    def redeem_recovery_ticket(self, data):
        token = data.get('ticket')
        if not isinstance(token, str) or not re.fullmatch(r'[a-zA-Z0-9_-]{43}', token):
            raise Rejected('ticket_unavailable', 'Recovery ticket is unavailable.', 404)
        token_hash = hashlib.sha256(token.encode('ascii')).hexdigest()
        now = time.time()
        ticket = self.db.execute('SELECT * FROM recovery_ticket WHERE token_hash=?', (token_hash,)).fetchone()
        if not ticket or ticket['redeemed'] or ticket['expires_at'] <= now:
            raise Rejected('ticket_unavailable', 'Recovery ticket is unavailable.', 404)
        row = self.get(ticket['lease_id'])
        checkpoint = self.available_checkpoint(row, now, ticket['checkpoint_id'], ticket_held=True)
        with self.db:
            self.db.execute('UPDATE recovery_ticket SET redeemed=1, expires_at=? WHERE token_hash=?',
                            (now + RECOVERY_TICKET_SECONDS, token_hash))
        return {field: checkpoint[field] for field in ('snapshot', 'path', 'sha256')}

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
        elif phase == 'ready' and time.time() >= row['deadline'] + self.grace_seconds and 'running' not in (row['hold_mode'], row['reviewer_hold']):
            if self.provider.status() == 'stopped':
                self.request_export(key)
            else:
                self.dispatch(key, 'export-stopping', self.provider.shutdown)
        elif phase in ('export-stopping', 'resetting'):
            self.inspect(key, row['owner'])
        elif phase == 'awaiting-export' and row['checkpoint'] and not (row['hold_mode'] or row['reviewer_hold']):
            if not json.loads(row['checkpoint']).get('restore_verified'):
                # Re-verify the same storage operation with the current worker before reuse.
                with self.db:
                    self.db.execute('UPDATE lease SET checkpoint=NULL WHERE id=?', (key,))
                return
            self.check_export_source(row)
            self.dispatch(key, 'resetting', self.provider.reset)

    def release(self, lease, data, owner):
        row = self.owned(lease, owner)
        if row['phase'] in ('released', 'stopping', 'export-stopping', 'awaiting-export', 'resetting'):
            return self.inspect(lease, owner)
        if row['phase'] != 'ready':
            raise Rejected('lease_not_releasable', 'Inspect the start operation before requesting release.')
        if row['reclaim_on_expiry']:
            if data.get('preserve_checkpoint', True) is not True or data.get('discard_disposable_data'):
                raise Rejected('final_export_required', 'Enrolled leases require verified export before reset.')
            if row['hold_mode'] or row['reviewer_hold']:
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
    parser.add_argument('--enroll-ledger', action='store_true')
    args = parser.parse_args()
    config = json.loads(args.config.read_text())
    if config['database'] == ':memory:':
        raise ValueError('The service requires a durable ledger.')
    controller = Pilot(config, enroll=args.enroll_ledger)
    if args.enroll_ledger:
        controller.db.close()
        print('Idle slot and independent ledger witness enrolled.')
        return
    task_tokens = {owner: Path(path).read_text().strip()
               for owner, path in config['client_tokens'].items()}
    if (not task_tokens or any(not re.fullmatch(r'[a-zA-Z0-9_-]{1,64}', owner)
                           for owner in task_tokens)
            or any(not re.fullmatch(r'[a-zA-Z0-9_-]{32,128}', value)
                   for value in task_tokens.values())
            or len(set(task_tokens.values())) != len(task_tokens)):
        raise ValueError('Configure a different randomly generated token for each pilot principal')

    verifier_ref = config.get('reclamation', {}).get('verifier_token_file')
    verifier = Path(verifier_ref).read_text().strip() if verifier_ref else None
    if verifier is not None and (not re.fullmatch(r'[a-zA-Z0-9_-]{32,128}', verifier)
                                 or verifier in task_tokens.values()):
        raise ValueError('Use a distinct final export verifier credential')

    recovery_ref = config.get('recovery', {}).get('worker_token_file')
    recovery_token = Path(recovery_ref).read_text().strip() if recovery_ref else None
    if recovery_token is not None and (not re.fullmatch(r'[a-zA-Z0-9_-]{32,128}', recovery_token)
            or recovery_token == verifier or recovery_token in task_tokens.values()):
        raise ValueError('Use a distinct recovery worker credential')

    reservation_tokens = {owner: Path(path).read_text().strip()
                          for owner, path in config.get('reservation_tokens', {}).items()}
    issuer_tokens = {issuer: Path(path).read_text().strip()
                     for issuer, path in config.get('task_issuer_tokens', {}).items()}
    review_ref = config.get('review_token_file')
    reviewer = Path(review_ref).read_text().strip() if review_ref else None
    if reviewer is not None and not re.fullmatch(r'[a-zA-Z0-9_-]{32,128}', reviewer):
        raise ValueError('Configure a dedicated read-only lease review credential')
    alert_ref = config.get('reclamation', {}).get('alert_token_file')
    alert_token = Path(alert_ref).read_text().strip() if alert_ref else None
    if alert_token is not None and (not verifier or not re.fullmatch(r'[a-zA-Z0-9_-]{32,128}', alert_token)):
        raise ValueError('Export alerts require a dedicated token and a configured export verifier')
    all_tokens = [*task_tokens.values(), *reservation_tokens.values(), *issuer_tokens.values()]
    all_tokens.extend(token for token in (recovery_token, verifier, reviewer, alert_token) if token)
    if (any(not re.fullmatch(r'[a-zA-Z0-9_-]{1,64}', owner) for owner in (*reservation_tokens, *issuer_tokens))
            or any(not re.fullmatch(r'[a-zA-Z0-9_-]{32,128}', value)
                   for value in (*reservation_tokens.values(), *issuer_tokens.values()))
            or len(set(all_tokens)) != len(all_tokens)):
        raise ValueError('Configure separate credentials for reservation and task issuer principals')

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
            for principal, value in task_tokens.items():
                if hmac.compare_digest(supplied, ('Bearer ' + value).encode('ascii')):
                    owner = principal
            header = self.headers.get('Authorization', '')
            if owner is None and header.startswith('Bearer '):
                owner = controller.task_credentials.owner(header.removeprefix('Bearer '))
            is_verifier = verifier is not None and hmac.compare_digest(supplied, ('Bearer ' + verifier).encode('ascii'))
            target = urllib.parse.urlsplit(self.path)
            if target.path == '/v1/lease-review':
                if reviewer is None or not hmac.compare_digest(supplied, ('Bearer ' + reviewer).encode('ascii')):
                    self.respond({'code': 'unauthorized'}, 401)
                    return
                if self.command != 'GET':
                    self.respond({'code': 'method_not_allowed'}, 405)
                    return
                try:
                    query = urllib.parse.parse_qs(target.query, keep_blank_values=True)
                    cursor, = query.pop('before', [None])
                    scope, = query.pop('scope', ['all'])
                    if query:
                        raise ValueError()
                    page = lease_page(controller.db, cursor, scope)
                except Rejected as error:
                    self.respond({'code': error.code}, 400)
                    return
                except ValueError:
                    self.respond({'code': 'invalid_query'}, 400)
                    return
                self.respond(page)
                return
            parts = self.path.strip('/').split('/')
            delegated_route = len(parts) == 3 and parts[:2] == ['v1', 'delegated-renewal']
            export_route = len(parts) in (2, 3) and parts[:2] == ['v1', 'final-export']
            expiry_route = parts == ['v1', 'checkpoint-expiry']
            alert_route = parts == ['v1', 'export-alerts']
            is_alert_worker = alert_token is not None and hmac.compare_digest(
                supplied, ('Bearer ' + alert_token).encode('ascii'))
            recovery_route = len(parts) == 2 and parts[0] == 'v1' and parts[1] in (
                'redeem-checkpoint', 'redeem-upload', 'complete-upload')
            is_recovery_worker = recovery_token is not None and hmac.compare_digest(
                supplied, ('Bearer ' + recovery_token).encode('ascii'))
            reservation_route = len(parts) >= 2 and parts[:2] == ['v1', 'capacity-reservations']
            reservation_owner = next((principal for principal, value in reservation_tokens.items()
                if hmac.compare_digest(supplied, ('Bearer ' + value).encode('ascii'))), None)
            issue_route = parts == ['v1', 'task-credentials']
            issuer = next((name for name, value in issuer_tokens.items()
                if hmac.compare_digest(supplied, ('Bearer ' + value).encode('ascii'))), None)
            authorized = (issuer is not None if issue_route
                          else reservation_owner is not None if reservation_route
                          else is_recovery_worker if recovery_route
                          else is_alert_worker if alert_route
                          else is_verifier if export_route or expiry_route else owner is not None)
            if not authorized and not (delegated_route and self.command == 'POST'):
                self.respond({'code': 'unauthorized'}, 401)
                return
            try:
                if parts == ['v1','capabilities'] and self.command == 'GET':
                    controller.provider.verified_config()
                    observation = controller.capacity.read_observation(allow_running_slot=True)
                    self.respond({'recipe': RECIPE, 'observed_at': time.time(),
                                  'capabilities': {**PLATFORM, 'gpu': 'none', **observation['slot_resources']}})
                    return
                if reservation_route and self.command == 'GET' and len(parts) == 2:
                    self.respond({'reservations': controller.reservations.list(reservation_owner)})
                    return
                if alert_route and self.command == 'GET':
                    self.respond(controller.next_export_alert())
                    return
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
                    if alert_route:
                        self.respond(controller.acknowledge_export_alert(data))
                        return
                    if issue_route:
                        self.respond(controller.task_credentials.issue(issuer, data))
                        return
                    if reservation_route and len(parts) == 2:
                        self.respond(controller.reservations.reserve(reservation_owner, data))
                        return
                    if reservation_route and len(parts) == 4 and parts[3] == 'release':
                        self.respond(controller.reservations.release(reservation_owner, parts[2]))
                        return
                    if delegated_route:
                        header = self.headers.get('Authorization', '')
                        token = header.removeprefix('Bearer ') if header.startswith('Bearer ') else ''
                        self.respond(controller.delegated_renew(parts[2], token))
                        return
                    if recovery_route:
                        recovery_operation = {'redeem-checkpoint': controller.redeem_recovery_ticket,
                                              'redeem-upload': controller.redeem_upload,
                                              'complete-upload': controller.complete_upload}
                        self.respond(recovery_operation[parts[1]](data))
                        return
                    if expiry_route:
                        self.respond(controller.confirm_checkpoint_expiry(data) if data
                                     else controller.claim_expired_checkpoint())
                        return
                    if parts == ['v1', 'leases']:
                        self.respond(controller.acquire(data, owner), 202)
                        return
                    if len(parts) == 4 and parts[:2] == ['v1', 'leases']:
                        if parts[3] == 'restore-complete':
                            self.respond(controller.finish_restore(parts[2], owner, data))
                            return
                        if parts[3] == 'delegate-renewal':
                            self.respond(controller.delegate_renewal(parts[2], data, owner))
                            return
                        if parts[3] == 'renew':
                            self.respond(controller.renew(parts[2], data, owner))
                            return
                        if parts[3] == 'reviewer':
                            self.respond(controller.assign_reviewer(parts[2], data, owner))
                            return
                        if parts[3] == 'reviewer-hold':
                            self.respond(controller.hold(parts[2], data, owner, delegated=True))
                            return
                        if parts[3] == 'override-reviewer':
                            self.respond(controller.override_review_hold(parts[2], data, owner))
                            return
                        if parts[3] == 'hold':
                            self.respond(controller.hold(parts[2], data, owner))
                            return
                        if parts[3] == 'export-disposition':
                            self.respond(controller.decide_failed_export(parts[2], data, owner))
                            return
                        if parts[3] == 'retry-export':
                            self.respond(controller.retry_export(parts[2], owner))
                            return
                        if parts[3] == 'checkpoint-recovery':
                            self.respond(controller.recovery_ticket(parts[2], owner, data))
                            return
                        if parts[3] == 'checkpoint-upload':
                            self.respond(controller.grant_upload(parts[2], data, owner))
                            return
                        if parts[3] == 'checkpoint-pin':
                            self.respond(controller.set_pin(parts[2], data, owner))
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
