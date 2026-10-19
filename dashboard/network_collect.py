"""Read-only collector, sent over pinned SSH; never installs files on targets."""
import concurrent.futures
import glob
import ipaddress
import json
import os
import platform
import plistlib
import re
import subprocess


def run(args):
    p = subprocess.run(args, capture_output=True, text=True, timeout=20)
    if p.returncode: raise RuntimeError('Read failed')
    return p.stdout


def linux():
    links = json.loads(run(['ip', '-j', '-4', 'addr']))
    macs = {v['ifname']: v.get('address', '') for v in json.loads(run(['ip', '-j', 'link']))}
    for link in links: link['address'] = macs.get(link['ifname'], '')
    rows = []
    by_ip = {}
    modes = {}
    paths = ['/etc/network/interfaces', '/etc/network/interfaces.new']
    for pattern in ['/etc/network/interfaces.d/*', '/etc/netplan/*', '/etc/systemd/network/*.network', '/etc/NetworkManager/system-connections/*', '/etc/cloud/cloud.cfg.d/*network*']:
        paths.extend(glob.glob(pattern))
    static = []
    for path in paths:
        if not os.path.isfile(path): continue
        text = open(path).read()
        if '/netplan/' in path or '/cloud/' in path:
            if re.search(r'addresses:|dhcp4:\s*(false|no)', text):
                rows.append({'reason': 'Saved YAML network profile needs structured validation', 'source': path})
            continue
        interface = None
        for line in text.splitlines():
            line = line.strip()
            if not line or line.startswith('#'): continue
            m = re.match(r'iface\s+(\S+)\s+inet\s+(\S+)', line)
            if m: interface = m[1]; modes[interface] = m[2]
            # Only address declarations, never DNS, routes, gateways or credential fields.
            if re.match(r'(?i)(address(?:es|\d+)?\s*[:=\s]|ip_address\s*=)', line):
                for addr in re.findall(r'\b(?:\d{1,3}\.){3}\d{1,3}\b', line.split(',')[0]):
                    static.append((addr, interface, path))
        # Multiline YAML address lists are not silently treated as covered.
        if re.search(r'addresses:\s*\n\s*-', text):
            rows.append({'reason': 'Multiline YAML addresses require interface mapping review', 'source': path})
    for link in links:
        for addr in link.get('addr_info', []):
            if addr.get('family') != 'inet': continue
            ip = addr['local']; by_ip[ip] = link
            mode = 'dhcp' if addr.get('dynamic') or 'dynamic' in addr.get('flags', []) or modes.get(link['ifname']) == 'dhcp' else 'static' if modes.get(link['ifname']) == 'static' else 'unknown'
            rows.append({'address': ip, 'mac': link.get('address', ''), 'interface': link['ifname'], 'assignment': mode, 'source': 'live interfaces'})
    for ip, interface, path in static:
        link = next((v for v in links if v['ifname'] == interface), None) if interface else by_ip.get(ip)
        rows.append({'address': ip, 'mac': link.get('address', '') if link else '', 'interface': interface, 'assignment': 'static', 'source': path})
    if not rows: rows.append({'reason': 'No interface evidence collected'})
    return rows


def macos():
    config = plistlib.load(open('/Library/Preferences/SystemConfiguration/preferences.plist', 'rb'))
    modes = {}; saved = []
    for service in config.get('NetworkServices', {}).values():
        name = service.get('Interface', {}).get('DeviceName')
        ipv4 = service.get('IPv4', {})
        if name:
            modes[name] = ipv4.get('ConfigMethod')
            for address in ipv4.get('Addresses', []): saved.append((name, address))
    rows = []; macs = {}; current = None
    for line in run(['/sbin/ifconfig', '-a']).splitlines():
        if line and not line[0].isspace(): current = line.split(':')[0]
        m = re.search(r'ether ([0-9a-f:]+)', line)
        if m: macs[current] = m[1]
        m = re.search(r'inet (\d+\.\d+\.\d+\.\d+)', line)
        if m:
            address = m[1]; mode = 'unknown'
            if modes.get(current) == 'Manual': mode = 'static'
            elif modes.get(current) == 'DHCP':
                try:
                    packet = run(['/usr/sbin/ipconfig', 'getpacket', current])
                    mode = 'dhcp' if re.search(r'yiaddr\s*=\s*' + re.escape(address) + r'\b', packet) else 'static'
                except Exception: mode = 'unknown'
            rows.append({'address': address, 'interface': current, 'assignment': mode, 'source': 'live macOS interfaces and DHCP lease'})
    for name, address in saved: rows.append({'address': address, 'interface': name, 'assignment': 'static', 'source': 'saved macOS network service'})
    for row in rows: row['mac'] = macs.get(row['interface'], '')
    return rows

def main():
    try: rows = macos() if platform.system() == 'Darwin' else linux()
    except Exception: rows = [{'reason': 'Platform/configuration collector unavailable'}]
    # Host scope includes guests, including those not yet registered individually.
    if os.path.exists('/usr/sbin/qm'):
        guests = json.loads(run(['pvesh', 'get', '/cluster/resources', '--type', 'vm', '--output-format', 'json']))
        def guest_rows(guest):
            gid = str(guest['vmid']); suffix = '/' + guest['type'] + '/' + gid
            try:
                if guest.get('status') != 'running': raise RuntimeError('Offline')
                code = globals()['GUEST_SOURCE']
                if guest['type'] == 'qemu':
                    value = json.loads(run(['qm', 'guest', 'exec', gid, '--', 'python3', '-c', code]))
                    if value.get('exitcode') != 0 or value.get('out-truncated'): raise RuntimeError('Guest collection failed')
                    result = json.loads(value['out-data'])
                else:
                    result = json.loads(run(['pct', 'exec', gid, '--', 'python3', '-c', code]))
                for row in result: row['suffix'] = suffix
                return result
            except Exception:
                return [{'suffix': suffix, 'reason': 'Guest live/saved configuration unavailable (stopped, unsupported or agent inaccessible)', 'runtime': guest.get('status')}]
        with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
            for result in pool.map(guest_rows, guests): rows.extend(result)
    print(json.dumps(rows))

if __name__ == '__main__':
    if globals().get('GUEST_ONLY'): print(json.dumps(linux()))
    else: main()
