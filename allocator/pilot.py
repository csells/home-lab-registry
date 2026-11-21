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


PROVIDER_TASK_TYPES = {
    'starting': 'qmstart',
    'stopping': 'qmshutdown',
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

    def call(self, route, fields=None):
        data = None if fields is None else urllib.parse.urlencode(fields).encode()
        request = urllib.request.Request(self.base + route, data=data,
                                         headers={'Authorization': 'PVEAPIToken=' + self.token})
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
            ):
                if column not in columns:
                    self.db.execute(f'ALTER TABLE lease ADD COLUMN {column} {definition}')
        self.provider = Proxmox(config['proxmox'])
        self.user = config['guest_user']
        self.host_key = public_key(config['guest_host_key'])

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
        if phase in ('starting', 'stopping', 'resetting'):
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
        elif phase == 'resetting':
            if self.provider.status() != 'stopped':
                raise Rejected('unexpected_guest_state', 'The reset slot did not return to the stopped state.')
            row = self.update(lease, 'released')
        status = row['phase']
        if status == 'ready' and time.time() >= row['deadline']:
            status = 'overdue'
        return self.public(row, status)

    def public(self, row, status=None):
        result = {key: value for key, value in row.items() if key not in ('prior_tasks', 'ssh_key', 'request_key')}
        result['status'] = status or row['phase']
        if row['address']:
            result['ssh'] = {'host': row['address'], 'user': self.user,
                             'host_key_alias': self.provider.name, 'host_key': self.host_key}
        return result

    def acquire(self, data, owner):
        task, key, seconds = data.get('task'), data.get('request_key'), data.get('seconds')
        if not all(isinstance(s, str) and 0 < len(s) <= 128 for s in (task, key)):
            raise Rejected('invalid_request', 'Provide a task and a request_key of 1–128 characters.', 400)
        self.check_duration(seconds)
        key = owner + '/' + key
        ssh_key = public_key(data.get('ssh_public_key'))
        if data.get('recipe') != 'debian13-amd64':
            raise Rejected('unsupported_recipe', 'The pilot provides only debian13-amd64.', 422)
        prev = self.db.execute('SELECT * FROM lease WHERE request_key=?', (key,)).fetchone()
        if prev:
            if prev['task'] != task or prev['initial_seconds'] != seconds or prev['ssh_key'] != ssh_key:
                raise Rejected('request_key_conflict', 'This request key is already bound to different inputs.')
            return self.inspect(prev['id'], owner)
        if self.db.execute("SELECT 1 FROM lease WHERE phase!='released'").fetchone():
            raise Rejected('capacity_exhausted', 'The single pilot slot is already reserved.')
        if self.provider.status() != 'stopped':
            raise Rejected('slot_unavailable', 'The pilot guest is already running outside the controller.')
        lease = uuid.uuid4().hex
        prior = json.dumps(sorted(self.provider.recent_tasks('starting')))
        with self.db:
            self.db.execute('''INSERT INTO lease
                (id, request_key, task, initial_seconds, phase, deadline, prior_tasks, owner, ssh_key)
                VALUES (?,?,?,?,?,?,?,?,?)''',
                            (lease, key, task, seconds, 'starting', time.time()+seconds, prior, owner, ssh_key))
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

    def release(self, lease, data, owner):
        row = self.owned(lease, owner)
        if row['phase'] in ('released', 'stopping', 'resetting'):
            return self.inspect(lease, owner)
        if row['phase'] != 'ready':
            raise Rejected('lease_not_releasable', 'Inspect the start operation before requesting release.')
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
            if owner is None:
                self.respond({'code': 'unauthorized'}, 401)
                return
            try:
                parts = self.path.strip('/').split('/')
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
                    if parts == ['v1', 'leases']:
                        self.respond(controller.acquire(data, owner), 202)
                        return
                    if len(parts) == 4 and parts[:2] == ['v1', 'leases']:
                        if parts[3] == 'renew':
                            self.respond(controller.renew(parts[2], data, owner))
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

    print('Lease pilot listening on loopback', flush=True)
    HTTPServer(('127.0.0.1', int(config.get('port', 8093))), Handler).serve_forever()


if __name__ == '__main__':
    main()
