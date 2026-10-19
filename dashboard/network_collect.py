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
from pathlib import Path


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
        text = Path(path).read_text()
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

def saved_profiles(root, mac, allow_empty=False):
    """Read only network settings; require a positive supported profile."""
    import configparser
    rows=[]
    def row(mode,address=None,source=''):
        if address:
            address=address.split('/')[0]
            try: ipaddress.IPv4Address(address)
            except ValueError:return
        rows.append({'assignment':mode,'address':address,'mac':mac,'scope':'offline','profile_verified':True,'source':source})
    for path in glob.glob(root+'/etc/netplan/*.yaml'):
        import yaml
        value=yaml.safe_load(Path(path).read_text()) or {}
        for kind in ('ethernets','bridges','vlans','bonds','wifis'):
            for name,cfg in value.get('network',{}).get(kind,{}).items():
                if cfg.get('dhcp4') is True:row('dhcp',source=path.removeprefix(root))
                for address in cfg.get('addresses',[]):row('static',address,path.removeprefix(root))
    for pattern in ('/etc/network/interfaces','/etc/network/interfaces.new','/etc/network/interfaces.d/*'):
        for path in glob.glob(root+pattern):
            if not os.path.isfile(path):continue
            for line in Path(path).read_text().splitlines():
                line=line.strip()
                if line.startswith('#'):continue
                m=re.match(r'iface\s+\S+\s+inet\s+(dhcp|static)\b',line)
                if m and m[1]=='dhcp':row('dhcp',source=path.removeprefix(root))
                m=re.match(r'address\s+(\S+)',line)
                if m:row('static',m[1],path.removeprefix(root))
    for pattern in ('/etc/systemd/network/*.network','/etc/NetworkManager/system-connections/*'):
        for path in glob.glob(root+pattern):
            parser=configparser.ConfigParser(strict=False,interpolation=None)
            parser.read(path)
            source=path.removeprefix(root)
            if parser.has_section('ipv4'):
                method=parser.get('ipv4','method',fallback='')
                if method=='auto':row('dhcp',source=source)
                for key,value in parser.items('ipv4'):
                    if re.fullmatch(r'address\d+',key):row('static',value.split(',')[0],source)
            if parser.has_section('Network'):
                if parser.get('Network','DHCP',fallback='').lower() in ('yes','true','ipv4'):row('dhcp',source=source)
                # Parse repeated Address= lines without ConfigParser's last-value loss.
                for line in Path(path).read_text().splitlines():
                    if line.strip().startswith('Address='):row('static',line.split('=',1)[1].strip(),source)
    if not rows and not allow_empty:raise RuntimeError('No supported saved IPv4 profile found')
    return rows


def offline_guest(guest):
    import tempfile,fcntl
    gid=str(int(guest['vmid']));kind=guest['type']; tool='qm' if kind=='qemu' else 'pct'
    config=json.loads(run(['pvesh','get','/nodes/'+run(['hostname']).strip()+'/'+kind+'/'+gid+'/config','--output-format','json']))
    macs=[]
    for key,value in config.items():
        if re.fullmatch(r'net\d+',key):
            m=re.search(r'(?:virtio|e1000|e1000e|rtl8139|vmxnet3|hwaddr)=([0-9a-fA-F:]{17})',value)
            if m:macs.append(m[1].lower())
    if len(macs)!=1:raise RuntimeError('Offline interface mapping needs one NIC or explicit MAC mapping')
    disk=config.get('rootfs') if kind=='lxc' else next((config[k] for k in ['scsi0','virtio0','sata0'] if k in config),None)
    if not disk:raise RuntimeError('No supported system disk')
    volume=disk.split(',')[0];dev=run(['pvesm','path',volume]).strip()
    loop=None;mount=None;mounted=False;was_inactive=False
    try:
        if dev.startswith('/dev/'):
            was_inactive=run(['lvs','--noheadings','-o','lv_active',dev]).strip()!='active'
            if was_inactive:run(['lvchange','-ay','-K',dev])
        probe=subprocess.run(['blkid','-p','-s','TYPE','-o','value',dev],capture_output=True,text=True,timeout=20)
        if probe.returncode not in (0,2):raise RuntimeError('Disk probe failed')
        typ=probe.stdout.strip()
        offset=0
        if typ!='ext4':
            table=json.loads(run(['sfdisk','--json',dev]))['partitiontable']
            partition=max(table['partitions'],key=lambda p:p['size']);offset=partition['start']*table['sectorsize']
        loop=run(['losetup','--find','--show','--read-only','--offset',str(offset),dev]).strip()
        if run(['blkid','-p','-s','TYPE','-o','value',loop]).strip()!='ext4':raise RuntimeError('Unsupported offline filesystem')
        mount=tempfile.mkdtemp(prefix='lab-address-read-')
        run(['mount','-o','ro,noload',loop,mount]);mounted=True
        rows=saved_profiles(mount,macs[0],allow_empty=bool(config.get('template') and config.get('ipconfig0') and not 'network=' in config.get('cicustom','')))
        if 'stopped' not in run([tool,'status',gid]):raise RuntimeError('Guest started during read; retry')
        for row in rows:row['runtime']='stopped';row['evidence']='Saved system disk read-only; ext4 journal replay disabled'
        # Cloud-init changes are relevant to next boot even when not yet applied.
        for key,value in config.items():
            if re.fullmatch(r'ipconfig\d+',key):
                m=re.search(r'(?:^|,)ip=([^,]+)',value)
                if m:
                    address=m[1];rows.append({'assignment':'dhcp' if address=='dhcp' else 'static','address':None if address=='dhcp' else address.split('/')[0],'mac':macs[0],'scope':'offline','profile_verified':True,'source':'Proxmox '+key})
        return rows
    finally:
        if mounted:run(['umount',mount])
        if mount:os.rmdir(mount)
        if loop:run(['losetup','-d',loop])
        if was_inactive and 'stopped' in run([tool,'status',gid]):run(['lvchange','-an',dev])


def ha_guest(gid):
    # HAOS intentionally lacks Python; NetworkManager has a supported CLI.
    def execute(args):
        value=json.loads(run(['qm','guest','exec',gid,'--']+args))
        if value.get('exitcode')!=0 or value.get('out-truncated'):raise RuntimeError('HA network read failed')
        return value.get('out-data','')
    links=json.loads(execute(['ip','-j','addr']))
    rows=[]
    for link in links:
        name=link['ifname']
        if name=='lo':continue
        for address in link.get('addr_info',[]):
            if address.get('family')=='inet':rows.append({'address':address['local'],'mac':link.get('address',''),'assignment':'dhcp' if address.get('dynamic') else 'unknown','interface':name,'source':'HAOS live interface'})
    uuids=execute(['nmcli','-g','UUID','connection','show']).splitlines()
    for uuid in uuids:
        method=execute(['nmcli','-g','ipv4.method','connection','show',uuid]).strip()
        iface=execute(['nmcli','-g','connection.interface-name','connection','show',uuid]).strip()
        mac=next((l.get('address','') for l in links if l['ifname']==iface),'')
        addresses=execute(['nmcli','-g','ipv4.addresses','connection','show',uuid]).strip()
        for address in addresses.split(','):
            if address:rows.append({'address':address.split('/')[0],'mac':mac,'assignment':'static','source':'HAOS saved NetworkManager profile'})
        if method=='auto':
            for row in rows:
                if row.get('interface')==iface:row['assignment']='dhcp'
    return rows


def tart_offline(name):
    import tempfile
    from pathlib import Path
    if '/' in name or name in ('.','..'):raise RuntimeError('Invalid Tart guest name')
    root=Path.home()/'.tart/vms'/name
    cfg=json.loads((root/'config.json').read_text());disk=None;mount=None
    try:
        attached=plistlib.loads(subprocess.check_output(['hdiutil','attach','-readonly','-nomount','-plist',str(root/'disk.img')],timeout=45))
        disk=next(e['dev-entry'] for e in attached['system-entities'] if e.get('content-hint')=='GUID_partition_scheme')
        devices={e['dev-entry'].removeprefix('/dev/') for e in attached['system-entities']}
        apfs=plistlib.loads(subprocess.check_output(['diskutil','apfs','list','-plist'],timeout=20))
        volume=next(v for c in apfs['Containers'] for v in c['Volumes'] if v['DeviceIdentifier'] in devices and 'Data' in v.get('Roles',[]))
        mount=tempfile.mkdtemp(prefix='lab-tart-read-')
        run(['diskutil','mount','readOnly','-mountPoint',mount,volume['DeviceIdentifier']])
        data=plistlib.loads(Path(mount+'/Library/Preferences/SystemConfiguration/preferences.plist').read_bytes());rows=[]
        for service in data.get('NetworkServices',{}).values():
            ipv4=service.get('IPv4',{});method=ipv4.get('ConfigMethod')
            if method=='DHCP':rows.append({'assignment':'dhcp','mac':cfg['macAddress'],'scope':'offline','profile_verified':True,'source':'Read-only Tart SystemConfiguration'})
            for address in ipv4.get('Addresses',[]):rows.append({'assignment':'static','address':address,'mac':cfg['macAddress'],'scope':'offline','profile_verified':True,'source':'Read-only Tart SystemConfiguration'})
        if not rows:raise RuntimeError('No supported Tart profile')
        return rows
    finally:
        if disk:run(['hdiutil','detach',disk])
        if mount:os.rmdir(mount)

def main():
    try: rows = macos() if platform.system() == 'Darwin' else linux()
    except Exception: rows = [{'reason': 'Platform/configuration collector unavailable'}]
    # Host scope includes guests, including those not yet registered individually.
    if os.path.exists('/usr/sbin/qm'):
        guests = json.loads(run(['pvesh', 'get', '/cluster/resources', '--type', 'vm', '--output-format', 'json']))
        def guest_rows(guest):
            gid = str(guest['vmid']); suffix = '/' + guest['type'] + '/' + gid
            try:
                if guest.get('status') != 'running':
                    result=offline_guest(guest)
                    for row in result:row['suffix']=suffix
                    return result
                code = globals()['GUEST_SOURCE']
                if guest['type'] == 'qemu':
                    probe=json.loads(run(['qm','guest','exec',gid,'--','/bin/sh','-c','command -v python3']))
                    value = json.loads(run(['qm', 'guest', 'exec', gid, '--', 'python3', '-c', code])) if probe.get('exitcode')==0 else {'exitcode':127}
                    if value.get('exitcode') != 0:
                        result=ha_guest(gid)
                    else:
                        if value.get('out-truncated'):raise RuntimeError('Truncated guest collection')
                        result = json.loads(value['out-data'])
                else:
                    result = json.loads(run(['pct', 'exec', gid, '--', 'python3', '-c', code]))
                for row in result: row['suffix'] = suffix
                return result
            except Exception as exc:
                return [{'suffix':suffix,'reason':'Guest check failed: '+str(exc),'runtime':guest.get('status')}]
        with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
            for result in pool.map(guest_rows, guests): rows.extend(result)
    if platform.system()=='Darwin':
        states={v['Name']:v['Running'] for v in json.loads(run(['/opt/homebrew/bin/tart','list','--format','json']))}
        for name in globals().get('TART_NAMES',[]):
            try:
                if states.get(name) is not False:raise RuntimeError('Tart guest not stopped; live guest access required')
                found=tart_offline(name)
            except Exception as exc:found=[{'reason':'Tart check failed: '+str(exc)}]
            for row in found:row['suffix']='/tart/'+name
            rows.extend(found)
    print(json.dumps(rows))

if __name__ == '__main__':
    if globals().get('GUEST_ONLY'): print(json.dumps(linux()))
    else: main()
