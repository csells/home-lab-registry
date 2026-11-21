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


def verified_address(binding, observation, now):
    """Admit one current address only after matching the configured guest identity."""
    vmid = binding['vmid']
    if type(vmid) is not int or vmid <= 0:
        raise ValueError('Invalid guest identity')
    guest_kind = binding.get('guest_kind', 'qemu')
    if guest_kind not in ('qemu', 'lxc') or observation.get('kind', 'qemu') != guest_kind:
        raise ValueError('Guest type changed')
    identity = 'uuid' if guest_kind == 'qemu' else 'volume_id'
    if (type(observation['vmid']) is not int or observation['vmid'] != vmid or
            not binding[identity] or observation[identity] != binding[identity]):
        raise ValueError('Guest identity changed')
    if observation['state'] != 'running':
        raise ValueError('Guest is not running')
    observed = observation['observed_at']
    if type(observed) not in (int, float) or not -2 <= now-observed <= 60:
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
    return candidates.pop()


def discover(base, config, spec):
    binding = spec['provider']
    if binding.get('kind') != 'proxmox-ssh':
        raise ValueError('Unsupported discovery provider')
    vmid = binding['vmid']
    if type(vmid) is not int or vmid <= 0:
        raise ValueError('Invalid guest identity')
    target = ssh_target(config, binding['host_id'], binding['username'])
    cmd = ssh_options(base, binding['key']) + ['-p', str(target['port']), '-o',
        'HostKeyAlias='+target['host_key_alias'], target['destination'], 'discover '+str(vmid)]
    response = subprocess.run(cmd, check=True, capture_output=True, timeout=30)
    address = verified_address(binding, json.loads(response.stdout), time.time())
    user = spec['username']
    if not re.fullmatch(r'[A-Za-z_][A-Za-z0-9_-]*', user):
        raise ValueError('Invalid SSH user')
    port = spec.get('port', 22)
    if type(port) is not int or not 1 <= port <= 65535:
        raise ValueError('Invalid SSH port')
    alias = spec['host_key_alias']
    if not re.fullmatch(r'[a-zA-Z0-9_-]+', alias):
        raise ValueError('Invalid SSH host identity')
    result = {'destination': user+'@'+address, 'port': port,
              'host_key_alias': alias, 'proxy': None}
    if 'hop_username' in binding:
        hop = ssh_target(config, binding['host_id'], binding['hop_username'])
        result['proxy'] = shlex.join(ssh_options(base, binding['hop_key']) + [
            '-p', str(hop['port']), '-o', 'HostKeyAlias='+hop['host_key_alias'],
            '-W', '%h:%p', hop['destination']])
    return result
