"""On-demand, read-only address audit. No timers, model calls or network writes."""
import concurrent.futures
import ipaddress
import json
import logging
import pathlib
import subprocess
import threading
import time
import urllib.request


def compare(rows, reservations, networks):
    nets = [ipaddress.ip_network(n) for n in networks]
    reserved = {(r['ip'], r['mac'].lower()): r.get('description', '') for r in reservations}
    results = []
    for item in rows:
        row = dict(item)
        address = row.get('address')
        if not address:
            row.update(status='unverified'); results.append(row); continue
        try:
            if not any(ipaddress.ip_address(address) in n for n in nets):
                continue
        except ValueError:
            row.update(status='unverified', reason='Invalid address evidence'); results.append(row); continue
        match = (address, row.get('mac', '').lower()) in reserved
        mode = row.get('assignment')
        if mode == 'static' and not match:
            row.update(status='violation' if row.get('mac') else 'unverified', reason='Static address has no matching reservation for this MAC' if row.get('mac') else 'Static address found; interface MAC unresolved')
        elif mode in ('static', 'dhcp'):
            row.update(status='pass', reason='Reservation matches this interface MAC' if match else 'DHCP; reservation optional')
        else:
            row.update(status='unverified', reason='Assignment method not verified')
        results.append(row)
    return results


def eero_get(config, resource):
    session = json.loads(pathlib.Path(config['session_file']).read_text())
    network = str(config['network_id'])
    if not network.isdigit(): raise ValueError('Invalid network ID')
    req = urllib.request.Request('https://api-user.e2ro.com/2.2/networks/' + network + '/' + resource,
                                 headers={'Cookie': 's=' + session['session_token'], 'User-Agent': 'eero-api-rs/0.1'})
    with urllib.request.urlopen(req, timeout=30) as response:
        data = json.load(response)
    if data.get('meta', {}).get('code', 200) != 200: raise ValueError('Router rejected read')
    return data['data']


def collect(target, private):
    # Target definitions are operator-owned files, never browser parameters.
    source = pathlib.Path(__file__).with_name('network_collect.py').read_text()
    script = ('GUEST_SOURCE = ' + repr('GUEST_ONLY = True\n' + source) + '\n' + source).encode()
    command = ['ssh', '-T', '-i', str(private / target['key_file']), '-o', 'BatchMode=yes',
               '-o', 'ConnectTimeout=8', '-o', 'StrictHostKeyChecking=yes',
               '-o', 'UserKnownHostsFile=' + str(private / target['known_hosts_file'])]
    if target.get('host_key_alias'):
        command += ['-o', 'HostKeyAlias=' + target['host_key_alias']]
    command += [target['destination'], 'sudo -n python3 -' if target.get('sudo') else 'python3 -']
    try:
        p = subprocess.run(command, input=script, capture_output=True, timeout=110)
        if p.returncode: raise ValueError('Collector failed')
        rows = json.loads(p.stdout)
        for row in rows: row['machine'] = target['id'] + row.get('suffix', '')
        return rows
    except Exception:
        logging.exception('Address collection failed for %s', target['id'])
        return [{'machine': target['id'], 'reason': 'Live collection failed; check pinned SSH access and collector availability', 'status': 'unverified'}]


class NetworkAudit:
    def __init__(self, private):
        self.private = pathlib.Path(private)
        self.lock = threading.Lock()
        self.state = {'status': 'idle', 'rows': [], 'message': 'Not checked yet'}
        self.started = 0

    def snapshot(self):
        with self.lock: return dict(self.state)

    def start(self):
        with self.lock:
            if self.state['status'] == 'running' or time.time() - self.started < 30: return dict(self.state)
            self.started = time.time()
            self.state = {'status': 'running', 'started_at': self.started, 'rows': [], 'message': 'Reading router reservations and machine configuration…'}
            threading.Thread(target=self._run, daemon=True).start()
            return dict(self.state)

    def _run(self):
        try:
            cfg = json.loads((self.private / 'network-audit.json').read_text())
            reservations = eero_get(cfg, 'reservations')
            from client import request
            reader = json.loads((self.private / 'reader.json').read_text())
            catalog = request(reader, 'GET', '/v1/services')['services']
            from urllib.parse import urlsplit
            by_id = {r['id']: r for r in catalog}
            for target in cfg['targets']:
                endpoint = by_id.get(target['id'], {}).get('spec', {}).get('endpoints', {}).get('ssh', '')
                address = urlsplit(endpoint).hostname
                if address:
                    ipaddress.ip_address(address)  # no shell or SSH-option interpretation
                    target['destination'] = target['destination'].split('@')[0] + '@' + address
            rows = []
            with concurrent.futures.ThreadPoolExecutor(max_workers=3) as pool:
                for found in pool.map(lambda t: collect(t, self.private), cfg['targets']): rows.extend(found)
            # Coverage is explicit: never turn an unreachable host, stopped guest or appliance into green.
            covered = {t['id'] for t in cfg['targets']}
            observations = {r['machine'] for r in rows}
            for record in catalog:
                spec = record['spec']
                if spec.get('lifecycle') != 'active': continue
                resources = spec.get('resources', {})
                parent = resources.get('parent_machine'); guest_id = resources.get('guest_id', resources.get('vmid'))
                if not parent and guest_id: parent = 'machine-' + spec.get('host', '')
                guest_covered = any(f'{parent}/{kind}/{guest_id}' in observations for kind in ('qemu', 'lxc'))
                if (resources.get('kind') in ('machine', 'virtual-machine', 'container') or 'machine' in spec.get('capabilities', [])) and record['id'] not in covered and not guest_covered:
                    rows.append({'machine': record['id'], 'reason': 'No direct live collector; guest observations may appear under its hypervisor', 'status': 'unverified'})
                audit = resources.get('network_authority_audit', {})
                for kind in ('kvm', 'amt'):
                    if kind in audit:
                        rows.append({'machine': record['id'] + '/' + kind, 'reason': 'Appliance configuration not refreshed by this collector; last recorded audit: ' + str(audit[kind].get('at', audit.get('at', 'unknown'))) if isinstance(audit[kind], dict) else 'Appliance configuration not freshly verified', 'status': 'unverified'})
            result = compare(rows, reservations, cfg['lan_cidrs'])
            violations = sum(r['status'] == 'violation' for r in result)
            unknown = sum(r['status'] == 'unverified' for r in result)
            value = {'status': 'violations' if violations else 'incomplete' if unknown else 'passed',
                     'rows': result, 'violations': violations, 'unverified': unknown, 'checked_at': time.time(),
                     'message': f'{violations} violations · {unknown} unverified checks',
                     'scope': 'Fresh router reservations and live/saved host configuration. Read-only. Unreachable, stopped, unsupported and appliance coverage is explicitly unverified.'}
        except Exception:
            logging.exception('Read-only address audit failed')
            value = {'status': 'error', 'rows': [], 'checked_at': time.time(), 'message': 'Audit failed; no clean result established. Check server access to router and registry.'}
        with self.lock: self.state = value
