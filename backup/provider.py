"""Resolve a guest from an authenticated, pool-scoped provider observation."""
import ipaddress
import json
import re
import shlex
import subprocess
import time
from resolve import ssh_target


def ssh_options(base, key='collector-key'):
    if not re.fullmatch(r'[a-zA-Z0-9_-]+', key):
        raise ValueError('Invalid credential reference')
    return ['ssh', '-i', str(base/'private'/key), '-o', 'IdentitiesOnly=yes',
            '-o', 'BatchMode=yes', '-o', 'StrictHostKeyChecking=yes',
            '-o', 'ConnectTimeout=10', '-o', 'UserKnownHostsFile='+str(base/'private/known_hosts')]


def discover(base, config, spec):
    binding = spec['provider']
    if binding.get('kind') != 'proxmox-ssh':
        raise ValueError('Unsupported discovery provider')
    vmid = binding['vmid']
    if type(vmid) is not int or vmid <= 0:
        raise ValueError('Invalid guest identity')
    target = ssh_target(config, binding['host_id'], binding['username'])
    cmd = ssh_options(base, binding['key']) + ['-p', str(target['port']), '-o',
        'HostKeyAlias='+target['host_key_alias'], target['destination'], 'discover']
    response = subprocess.run(cmd, check=True, capture_output=True, timeout=30)
    observation = json.loads(response.stdout)
    # The forced-command credential returns one authorized guest only. It has no
    # shell or arbitrary guest-exec interface and must observe the live provider.
    if observation['vmid'] != vmid or observation['uuid'] != binding['uuid']:
        raise ValueError('Guest identity changed')
    if observation['state'] != 'running':
        raise ValueError('Guest is not running')
    observed = observation['observed_at']
    if type(observed) not in (int, float) or not -2 <= time.time()-observed <= 60:
        raise ValueError('Stale provider observation')
    mac = binding['mac'].lower()
    candidates = set()
    networks = [ipaddress.ip_network(n) for n in binding['allowed_networks']]
    for interface in observation['interfaces']:
        if interface.get('hardware-address', '').lower() != mac:
            continue
        for address in interface.get('ip-addresses', []):
            ip = ipaddress.ip_address(address['ip-address'])
            if ip.version == 4 and any(ip in net for net in networks):
                candidates.add(str(ip))
    if len(candidates) != 1:
        raise ValueError('Guest address unavailable or ambiguous')
    user = spec['username']
    if not re.fullmatch(r'[A-Za-z_][A-Za-z0-9_-]*', user):
        raise ValueError('Invalid SSH user')
    port = spec.get('port', 22)
    if type(port) is not int or not 1 <= port <= 65535:
        raise ValueError('Invalid SSH port')
    alias = spec['host_key_alias']
    if not re.fullmatch(r'[a-zA-Z0-9_-]+', alias):
        raise ValueError('Invalid SSH host identity')
    result = {'destination': user+'@'+candidates.pop(), 'port': port,
              'host_key_alias': alias, 'proxy': None}
    if 'hop_username' in binding:
        hop = ssh_target(config, binding['host_id'], binding['hop_username'])
        result['proxy'] = shlex.join(ssh_options(base, binding['hop_key']) + [
            '-p', str(hop['port']), '-o', 'HostKeyAlias='+hop['host_key_alias'],
            '-W', '%h:%p', hop['destination']])
    return result
