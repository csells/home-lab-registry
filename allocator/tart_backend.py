"""Client for one enrolled Tart worker reached via an operator-pinned tunnel."""
import ipaddress
import json
import re
import urllib.error
import urllib.request
from pathlib import Path
from urllib.parse import urlsplit

from checkpoints import Rejected


APPLE_RECIPE = 'macos-xcode-arm64'
APPLE_PLATFORM = {'os': 'macos', 'architecture': 'arm64', 'distribution': 'macos'}
LEASE = re.compile(r'[0-9a-f]{32}')
TASK = re.compile(r'(clone|start|stop|delete):([0-9a-f]{32})')
PHASE = {'starting': 'start', 'stopping': 'stop', 'export-stopping': 'stop', 'deleting': 'delete'}
MIB = 1024 ** 2


class TartScope:
    def __init__(self, config):
        url = urlsplit(config['url'])
        if (url.scheme != 'http' or url.hostname != '127.0.0.1' or not url.port
                or url.username is not None or url.password is not None
                or url.path not in ('', '/') or url.query or url.fragment):
            raise ValueError('Use only the pinned loopback Tart worker tunnel.')
        self.url = url.geturl().rstrip('/')
        self.node = config['identity']
        if not re.fullmatch(r'[0-9a-f]{32}', self.node):
            raise ValueError('Pin the enrolled worker identity.')
        self.token = Path(config['token_file']).read_text().strip()
        self.bounds = config['shape']
        if (not re.fullmatch(r'[A-Za-z0-9_-]{43,128}', self.token)
                or set(self.bounds) != {'vcpus', 'memory_mib', 'disk_mib'}
                or any(type(size) is not int or size < 1 for size in self.bounds.values())):
            raise ValueError('Configure a private worker token and qualified recipe shape.')
        self.guest_id = 'apple-workspace'
        self.opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())

    def call(self, action, **fields):
        request = urllib.request.Request(self.url, json.dumps({'action': action, **fields}).encode(),
                                        headers={'Authorization': 'Bearer ' + self.token,
                                                 'Content-Type': 'application/json'})
        try:
            with self.opener.open(request, timeout=8) as response:
                result = json.loads(response.read(65536))
        except urllib.error.HTTPError as error:
            with error:
                details = json.loads(error.read(4096))
            raise Rejected(details['code'], details['reason'], error.code) from error
        except (OSError, ValueError) as error:
            raise Rejected('apple_worker_unreachable', 'The enrolled Tart worker did not respond.', 503) from error
        if result.get('worker') != self.node:
            raise Rejected('apple_worker_identity', 'The response did not match the enrolled Tart worker.')
        return result['value']

    def identity(self):
        return ['tart', self.node, self.bounds]

    @staticmethod
    def name(lease):
        if not LEASE.fullmatch(lease):
            raise ValueError('Invalid Apple lease identity')
        return 'lease-' + lease

    def verify_idle(self):
        self.call('idle')

    def check(self, reservations=(0, 0), *, require_slot=True, allow_running_slot=False):
        value = self.call('capacity', allow_running=allow_running_slot)
        if value['shape'] != self.bounds:
            raise Rejected('apple_recipe_changed', 'The worker recipe differs from the enrolled shape.')
        for field, amount, code in zip(('memory_after_start', 'storage_after_writes'), reservations,
                                       ('host_memory_headroom', 'host_storage_headroom'), strict=True):
            if value[field] - amount * MIB < value['floor_'+field]:
                raise Rejected(code, 'Apple host cannot preserve its configured headroom.')
        return {**value, 'slot_resources': self.bounds}

    def clone(self, lease):
        return self.call('dispatch', lease=lease, operation='clone')

    def tasks(self, phase, lease=None):
        return set(self.call('tasks', operation=phase, lease=lease))

    def task(self, ref):
        if not TASK.fullmatch(ref):
            raise Rejected('apple_operation_identity', 'Invalid Tart operation reference.')
        return self.call('task', ref=ref)

    def describe(self, lease):
        return self.call('describe', lease=lease)

    def bind(self, lease, pins):
        return TartGuest(self, lease, pins['generation'])


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


class TartGuest:
    def __init__(self, scope, lease, generation):
        self.scope, self.lease, self.pin = scope, lease, generation
        self.name = scope.name(lease)

    def call(self, action, **fields):
        return self.scope.call(action, lease=self.lease, generation=self.pin, **fields)

    def generation(self):
        return self.call('describe')['generation']

    def status(self):
        return self.call('status')

    def ip(self):
        address = self.call('address')
        return str(ipaddress.IPv4Address(address)) if address else None

    def ready_host_key(self):
        return self.call('host-key')

    def install_guest_key(self, key, lease):
        if lease != self.lease:
            raise ValueError('Lease key target differs from the owned guest')
        self.call('guest-key', key=key)

    def dispatch(self, operation):
        return self.call('dispatch', operation=operation)

    def start(self):
        return self.dispatch('start')

    def shutdown(self):
        return self.dispatch('stop')

    def delete(self, *, discard=False):
        return self.call('dispatch', operation='delete', discard=discard)

    def task(self, ref):
        return self.scope.task(ref)

    def matches_task(self, ref, phase):
        return ref == PHASE[phase] + ':' + self.lease

    def recent_tasks(self, phase):
        return self.scope.tasks(PHASE[phase], self.lease)

    def slot_start_tasks(self):
        return json.dumps(sorted(self.recent_tasks('starting')))
