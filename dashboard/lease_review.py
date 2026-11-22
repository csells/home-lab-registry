"""Fetch a page of lease metadata through an operator-pinned read-only endpoint."""
import json
import logging
import math
import re
import ssl
import time
import urllib.request
from contextlib import contextmanager
from http.client import HTTPException
from threading import Condition
from pathlib import Path
from urllib.parse import urlencode, urlsplit

from reconciliation import fresh


LEASE_NAME = re.compile(r'lease-[0-9a-f]{32}')


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def page(config_path, cursor, scope='all'):
    config = json.loads(config_path.read_text())
    origin = urlsplit(config['endpoint'])
    if (origin.scheme != 'https' or not origin.hostname or origin.username is not None
            or origin.password is not None or origin.query or origin.fragment or origin.path not in ('', '/')):
        raise ValueError('Configure the TLS allocator endpoint.')
    if scope not in ('all', 'current', 'released', 'counts', 'inventory', 'issues'):
        raise ValueError('Unknown lease scope.')
    query = {'scope': scope}
    if cursor is not None:
        if not re.fullmatch(r'[1-9][0-9]{0,17}', cursor):
            raise ValueError('Invalid history cursor.')
        query['before'] = cursor
    path = '/v1/lease-review?' + urlencode(query)
    token = Path(config['token_file']).read_text().strip()
    client = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect(),
        urllib.request.HTTPSHandler(context=ssl.create_default_context(cafile=config['ca_file'])))
    req = urllib.request.Request(origin.geturl().rstrip('/') + path, headers={'Authorization': 'Bearer ' + token})
    with client.open(req, timeout=8) as response:
        raw = response.read(256 * 1024 + 1)
    if len(raw) > 256 * 1024:
        raise ValueError('Lease review page exceeded the response budget.')
    return json.loads(raw)


class LeaseInventory:
    """Refresh lease joins on inventory identity/state changes and operator requests."""

    def __init__(self, config_path):
        self.path = config_path
        self.nodes = json.loads(config_path.read_text()).get('inventory_machines', {}) if config_path.is_file() else {}
        if (not isinstance(self.nodes, dict)
                or not all(isinstance(value, str) and value.startswith('machine-') for value in self.nodes.values())
                or len(set(self.nodes.values())) != len(self.nodes)):
            raise ValueError('Map each provider node to its distinct registry machine ID.')
        self.previous = None
        self.claims = {}
        self.unavailable = frozenset()
        self.retry_at = None
        self.retry_delay = 5

    def reconcile(self, reports, force=False, now=None):
        now = time.time() if now is None else now
        if not self.nodes:
            return {}, frozenset()
        machines = set(self.nodes.values())
        observed = frozenset(
            (machine, g['id'], g['name'], g['type'], g['state'])
            for machine, report in reports.items()
            if machine in machines and fresh(report, now)
            for g in report['entries']
            if g.get('type') == 'qemu' and LEASE_NAME.fullmatch(g.get('name') or '')
        )
        if force or self.previous != observed or (self.retry_at is not None and now>=self.retry_at):
            if self.previous != observed or force:
                self.retry_delay = 5
            self.retry_at = None
            self.previous = observed
            self.claims = {}
            self.unavailable = frozenset()
            if observed:
                try:
                    result = page(self.path, None, 'inventory')
                    self.claims = self.match(self.nodes, observed, result['guests'])
                    self.retry_delay = 5
                except (OSError, HTTPException, ValueError, TypeError, KeyError) as error:
                    logging.warning('Lease inventory unavailable: %s', type(error).__name__)
                    self.unavailable = frozenset(row[:4] for row in observed)
                    self.retry_at = now + self.retry_delay
                    self.retry_delay = min(self.retry_delay * 2, 60)
        return self.claims, self.unavailable

    @staticmethod
    def match(nodes, observed, guests):
        claims = {}
        observed_keys = {row[:4] for row in observed}
        if not isinstance(guests, list) or len(guests) > 8:
            raise ValueError('Invalid allocator inventory')
        for row in guests:
            if (not isinstance(row, dict) or row['type'] != 'qemu'
                    or not re.fullmatch(r'[1-9][0-9]{2,8}', row['id'])
                    or not LEASE_NAME.fullmatch(row['name']) or row['name'] != 'lease-' + row['lease']):
                raise ValueError('Invalid allocator guest identity')
            node = row['node']
            if node not in nodes:
                continue
            key = (nodes[node], row['id'], row['name'], row['type'])
            if key in claims:
                raise ValueError('Duplicate allocator guest identity')
            if key in observed_keys:
                claims[key] = row['lease']
        return claims

class LeaseIssueFeed:
    """Share one allocator issues read among all connected dashboard viewers."""

    def __init__(self, config, changed, interval=20):
        self.config = config
        self.changed = changed
        self.enabled = config.is_file()
        self.interval = interval
        self.condition = Condition()
        self.viewers = 0
        self.due = 0
        self.closed = False
        self.current = {'status':'checking'} if self.enabled else None

    @contextmanager
    def watch(self):
        with self.condition:
            if not self.viewers:
                self.due = 0
            self.viewers += 1
            self.condition.notify()
        try:
            yield
        finally:
            with self.condition:
                self.viewers -= 1
                self.condition.notify()

    def refresh(self):
        with self.condition:
            self.due = 0
            self.condition.notify()

    def snapshot(self):
        with self.condition:
            return self.current

    def close(self):
        with self.condition:
            self.closed = True
            self.condition.notify()

    def run(self):
        if not self.enabled:
            return
        while True:
            with self.condition:
                while not self.closed:
                    delay = self.due - time.monotonic()
                    if self.viewers and delay <= 0:
                        break
                    self.condition.wait(max(0, delay) if self.viewers else None)
                if self.closed:
                    return
                self.due = time.monotonic() + self.interval
            try:
                report = page(self.config, None, 'issues')
                current = {'status':'verified','issues':report['issues'],'counts':report['counts']}
                deadline = report.get('next_deadline')
                lease_clock = report['as_of']
                if not math.isfinite(lease_clock) or (deadline is not None and not math.isfinite(deadline)):
                    raise ValueError('Invalid allocator timestamp')
            except (OSError, HTTPException, ValueError, TypeError, KeyError) as error:
                logging.warning('Lease issues unavailable: %s', type(error).__name__)
                current = {'status':'unavailable'}
                deadline = None
            with self.condition:
                if deadline is not None:
                    self.due = min(self.due, time.monotonic()+max(0.1, deadline-lease_clock))
                comparable = {key:value for key,value in self.current.items() if key!='as_of'}
                if comparable != current:
                    self.current = {**current,'as_of':time.time()}
                    self.changed.set()
