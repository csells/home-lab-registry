#!/usr/bin/env python3
"""Forced-command observer: read only operator-allowlisted Proxmox guest identities.

The private policy contains a node name and guests mapping string IDs to qemu/lxc.
Install with a source-restricted SSH key whose forced command supplies --policy.
No caller-supplied shell, path, node, guest command or provider mutation is accepted.
"""
import argparse
import json
import os
import re
import subprocess
import time


def guest_request(command, guests):
    parts = command.split()
    if len(parts) != 2 or parts[0] != 'discover' or not re.fullmatch(r'[1-9][0-9]*', parts[1]):
        raise ValueError('Expected scoped discovery request')
    kind = guests.get(parts[1])
    if kind not in ('qemu', 'lxc'):
        raise ValueError('Guest is not authorized')
    return parts[1], kind


def output(command):
    return json.loads(subprocess.check_output(command, timeout=20))


def observe(node, vmid, kind):
    if not re.fullmatch(r'[A-Za-z0-9_-]+', node):
        raise ValueError('Invalid configured node')
    prefix = '/nodes/'+node+'/'+kind+'/'+vmid
    config = output(['pvesh', 'get', prefix+'/config', '--output-format', 'json'])
    status = output(['pvesh', 'get', prefix+'/status/current', '--output-format', 'json'])
    result = {'vmid': int(vmid), 'kind': kind, 'state': status['status'],
              'interfaces': []}
    if kind == 'qemu':
        result['uuid'] = re.search(r'(?:^|,)uuid=([^,]+)', config['smbios1'])[1]
        if result['state'] == 'running':
            result['interfaces'] = output(['qm', 'guest', 'cmd', vmid, 'network-get-interfaces'])
    else:
        result['volume_id'] = config['rootfs'].split(',')[0]
        if result['state'] == 'running':
            interfaces = output(['pct', 'exec', vmid, '--', 'ip', '-j', 'addr', 'show'])
            result['interfaces'] = [
                {'name': i['ifname'], 'hardware-address': i.get('address', ''),
                 'ip-addresses': [{'ip-address': a['local']} for a in i.get('addr_info', [])]}
                for i in interfaces]
    result['observed_at'] = time.time()
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--policy', required=True)
    args = parser.parse_args()
    with open(args.policy) as f:
        policy = json.load(f)
    vmid, kind = guest_request(os.environ.get('SSH_ORIGINAL_COMMAND', ''), policy['guests'])
    print(json.dumps(observe(policy['node'], vmid, kind)))


if __name__ == '__main__':
    main()
