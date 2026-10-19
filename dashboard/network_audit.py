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
            if row.get('scope')=='offline' and row.get('profile_verified') and row.get('mac') and row.get('assignment')=='dhcp':
                row.update(status='offline-config-verified',reason='Saved DHCP configuration read without booting');results.append(row);continue
            row.update(status='unverified'); results.append(row); continue
        try:
            if not any(ipaddress.ip_address(address) in n for n in nets):
                if row.get('scope')=='offline':
                    row.update(status='not-applicable',reason='Saved address is outside the home LAN; eero reservation not applicable');results.append(row)
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
        if row.get('scope')=='offline' and row['status']=='pass':row['status']='offline-config-verified'
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


def guest_identity(spec):
    r=spec.get('resources',{});audit=r.get('network_authority_audit',{})
    gid=r.get('guest_id',r.get('vmid',audit.get('guest_id')))
    if gid is None:gid=r.get('vm_name')
    parent=r.get('parent_machine')
    if not parent and gid is not None:parent='machine-'+audit.get('host',spec.get('host',''))
    return parent,str(gid) if gid is not None else None


def ssh_command(target,private):
    import shlex
    command = ['ssh', '-T', '-i', str(private / target['key_file']), '-o', 'BatchMode=yes',
               '-o', 'ConnectTimeout=8', '-o', 'StrictHostKeyChecking=yes',
               '-o', 'UserKnownHostsFile=' + str(private / target['known_hosts_file'])]
    if target.get('host_key_alias'):command += ['-o','HostKeyAlias='+target['host_key_alias']]
    if target.get('jump'):
        jump=ssh_command(target['jump'],private)
        command += ['-o','ProxyCommand='+shlex.join(jump[:-1]+['-W','%h:%p',jump[-1]])]
    return command+[target['destination']]


def collect(target, private):
    source = pathlib.Path(__file__).with_name('network_collect.py').read_text()
    script = ('TART_NAMES = '+repr(target.get('tart_names',[]))+'\nGUEST_SOURCE = ' + repr('GUEST_ONLY = True\n' + source) + '\n' + source).encode()
    command=ssh_command(target,private)+['sudo -n python3 -' if target.get('sudo') else 'python3 -']
    try:
        p = subprocess.run(command, input=script, capture_output=True, timeout=240)
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
            for target in cfg['targets']:
                target['tart_names']=[s['spec']['resources']['vm_name'] for s in catalog if s['spec'].get('host')==target['id'].removeprefix('machine-') and s['spec'].get('resources',{}).get('vm_name')]
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
                parent,guest_id = guest_identity(spec)
                guest_covered = any(f'{parent}/{kind}/{guest_id}' in observations for kind in ('qemu', 'lxc','tart'))
                if (resources.get('kind') in ('machine', 'virtual-machine', 'container') or 'machine' in spec.get('capabilities', [])) and record['id'] not in covered and not guest_covered:
                    rows.append({'machine': record['id'], 'reason': 'No direct live collector; guest observations may appear under its hypervisor', 'status': 'unverified'})
                audit = resources.get('network_authority_audit', {})
                for kind in ('kvm', 'amt'):
                    if kind in audit:
                        rows.append({'machine': record['id'] + '/' + kind, 'reason': 'Appliance configuration not refreshed by this collector; last recorded audit: ' + str(audit[kind].get('at', audit.get('at', 'unknown'))) if isinstance(audit[kind], dict) else 'Appliance configuration not freshly verified', 'status': 'unverified'})
            from appliance_network import collect_appliance
            appliance_credentials=json.loads((self.private/'appliance-credentials.json').read_text())
            for target in cfg.get('appliances',[]):
                machine,kind=target['id'].rsplit('/',1)
                spec=by_id.get(machine,{}).get('spec',{})
                access=spec.get('resources',{}).get('access',{})
                entry=access.get('console' if kind=='kvm' else kind,{})
                endpoint=spec.get('endpoints',{}).get(kind) or (entry.get('url') if isinstance(entry,dict) else entry)
                if not endpoint:raise ValueError('Appliance endpoint missing from registry')
                target['url']=endpoint
                rows=[r for r in rows if r['machine']!=target['id']]
                rows.extend(collect_appliance(target,self.private,appliance_credentials[target['id']]))
            result = compare(rows, reservations, cfg['lan_cidrs'])
            result=list({json.dumps(row,sort_keys=True):row for row in result}.values())
            violations = sum(r['status'] == 'violation' for r in result)
            unknown = sum(r['status'] == 'unverified' for r in result)
            warnings=sum(bool(r.get('warning')) for r in result)
            value = {'status': 'violations' if violations else 'incomplete' if unknown else 'passed',
                     'rows': result, 'violations': violations, 'unverified': unknown, 'warnings':warnings, 'checked_at': time.time(),
                     'message': f'{violations} violations · {unknown} unverified checks · {warnings} warnings',
                     'scope': 'Fresh router reservations, live host/guest and appliance settings, plus read-only saved configuration for stopped guests. Offline verification is distinct from live verification; inaccessible configurations remain unverified.'}
        except Exception:
            logging.exception('Read-only address audit failed')
            value = {'status': 'error', 'rows': [], 'checked_at': time.time(), 'message': 'Audit failed; no clean result established. Check server access to router and registry.'}
        with self.lock: self.state = value
