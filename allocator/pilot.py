"""Private, single-slot Linux lease pilot. Run on loopback in a disposable VM."""
import hmac
import json
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


class Proxmox:
    def __init__(self, config):
        self.base = config['url'].rstrip('/') + '/api2/json'
        url = urllib.parse.urlsplit(config['url'])
        if (url.scheme != 'https' or not url.hostname or url.username or
                url.password or url.path not in ('', '/') or url.query or url.fragment):
            raise ValueError('Proxmox requires HTTPS')
        self.node = config['node']
        self.guest = f'/nodes/{self.node}/qemu/{int(config["guest_id"])}'
        self.token = Path(config['token_file']).read_text().strip()
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

    def task(self, upid):
        path = f'/nodes/{self.node}/tasks/{urllib.parse.quote(upid, safe="")}/status'
        result = self.call(path)
        if result['status'] == 'running':
            return False
        if result.get('exitstatus') != 'OK':
            raise Rejected('provider_task_failed', 'The recorded provider task failed and needs operator review.')
        return True

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
        self.provider = Proxmox(config['proxmox'])
        self.user = config['guest_user']

    def update(self, lease, phase, upstream=None, address=None):
        with self.db:
            self.db.execute('UPDATE lease SET phase=?, upstream=?, address=? WHERE id=?',
                            (phase, upstream, address, lease))
        return self.get(lease)

    def get(self, lease):
        row = self.db.execute('SELECT * FROM lease WHERE id=?', (lease,)).fetchone()
        if row is None:
            raise Rejected('unknown_lease', 'No lease has this identifier.', 404)
        return dict(row)

    def dispatch(self, lease, phase, operation):
        self.update(lease, phase)
        # Persist intent before dispatch. A lost response leaves the slot held
        # and requires operator reconciliation. Never replay an ambiguous write.
        task = operation()
        if not isinstance(task, str) or not task.startswith('UPID:'):
            raise Rejected('operation_outcome_unknown', 'Proxmox returned no task identifier. The slot remains held.')
        return self.update(lease, phase, upstream=task)

    def inspect(self, lease):
        row = self.get(lease)
        phase = row['phase']
        if phase in ('starting', 'stopping', 'resetting'):
            if not row['upstream']:
                return {**row, 'status': 'outcome-unknown'}
            if not self.provider.task(row['upstream']):
                return {**row, 'status': phase}
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
                return {**row, 'status': 'waiting-for-guest-ip'}
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
        result = {**row, 'status': status}
        if row['address']:
            result['ssh'] = {'host': row['address'], 'user': self.user}
        return result

    def acquire(self, data):
        task, key, seconds = data.get('task'), data.get('request_key'), data.get('seconds')
        if not all(isinstance(s, str) and 0 < len(s) <= 128 for s in (task, key)):
            raise Rejected('invalid_request', 'Provide a task and a request_key of 1–128 characters.', 400)
        self.check_duration(seconds)
        if data.get('recipe') != 'debian13-amd64':
            raise Rejected('unsupported_recipe', 'The pilot provides only debian13-amd64.', 422)
        prev = self.db.execute('SELECT * FROM lease WHERE request_key=?', (key,)).fetchone()
        if prev:
            if prev['task'] != task or prev['initial_seconds'] != seconds:
                raise Rejected('request_key_conflict', 'This request key is already bound to different inputs.')
            return self.inspect(prev['id'])
        if self.db.execute("SELECT 1 FROM lease WHERE phase!='released'").fetchone():
            raise Rejected('capacity_exhausted', 'The single pilot slot is already reserved.')
        if self.provider.status() != 'stopped':
            raise Rejected('slot_unavailable', 'The pilot guest is already running outside the controller.')
        lease = uuid.uuid4().hex
        with self.db:
            self.db.execute('INSERT INTO lease VALUES (?,?,?,?,?,?,?,?)',
                            (lease, key, task, seconds, 'starting', time.time()+seconds, None, None))
        return self.dispatch(lease, 'starting', self.provider.start)

    @staticmethod
    def check_duration(seconds):
        if type(seconds) is not int or not 0 < seconds <= 86400 * 365:
            raise Rejected('invalid_duration', 'Provide a duration between one second and one year.', 400)

    def renew(self, lease, data):
        seconds = data.get('seconds')
        self.check_duration(seconds)
        row = self.get(lease)
        if row['phase'] not in ('starting', 'ready'):
            raise Rejected('lease_not_renewable', 'Only starting or ready leases can renew.')
        with self.db:
            self.db.execute('UPDATE lease SET deadline=? WHERE id=?',
                            (max(row['deadline'], time.time()+seconds), lease))
        return self.inspect(lease)

    def release(self, lease, data):
        row = self.get(lease)
        if row['phase'] in ('released', 'stopping', 'resetting'):
            return self.inspect(lease)
        if row['phase'] != 'ready':
            raise Rejected('lease_not_releasable', 'Inspect the start operation before requesting release.')
        if data.get('discard_disposable_data') is not True:
            raise Rejected('release_requires_disposition', 'Confirm discard of the pilot workspace after saving any required data.', 400)
        if self.provider.status() == 'stopped':
            return self.dispatch(lease, 'resetting', self.provider.reset)
        return self.dispatch(lease, 'stopping', self.provider.shutdown)


def main():
    import argparse
    parser = argparse.ArgumentParser()
    parser.add_argument('config', type=Path)
    config = json.loads(parser.parse_args().config.read_text())
    controller = Pilot(config)
    secret = Path(config['client_token_file']).read_text().strip()
    if len(secret) < 32:
        raise ValueError('Use a randomly generated pilot client token')

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
            if not hmac.compare_digest(self.headers.get('Authorization', ''), 'Bearer ' + secret):
                self.respond({'code': 'unauthorized'}, 401)
                return
            try:
                parts = self.path.strip('/').split('/')
                if self.command == 'GET' and len(parts) == 3 and parts[:2] == ['v1', 'leases']:
                    self.respond(controller.inspect(parts[2]))
                    return
                if self.command == 'POST':
                    size = int(self.headers.get('Content-Length', '0'))
                    if not 0 < size <= 4096:
                        raise Rejected('invalid_body', 'Provide a JSON object up to 4096 bytes.', 400)
                    data = json.loads(self.rfile.read(size))
                    if not isinstance(data, dict):
                        raise Rejected('invalid_body', 'Provide a JSON object.', 400)
                    if parts == ['v1', 'leases']:
                        self.respond(controller.acquire(data), 202)
                        return
                    if len(parts) == 4 and parts[:2] == ['v1', 'leases']:
                        if parts[3] == 'renew':
                            self.respond(controller.renew(parts[2], data))
                            return
                        if parts[3] == 'release':
                            self.respond(controller.release(parts[2], data), 202)
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
