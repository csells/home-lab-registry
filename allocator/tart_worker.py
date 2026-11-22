"""Serve one staging recipe on a Mac through a private, pinned SSH tunnel."""
import argparse
import hashlib
import hmac
import ipaddress
import json
import os
import re
import shutil
import stat
import subprocess
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

from checkpoints import Rejected
from restore_workspace import MAX_ARCHIVE
from tart_backend import LEASE, MIB, TASK
from workspace_archive import archive_workspace

SHARE = '/Volumes/My Shared Files/workspace'
SSH_KEY = re.compile(r'ssh-ed25519 [A-Za-z0-9+/]{68}')
PREPARE_SSH = r'''
set -eu
umask 077
marker="$HOME/.lease-prepared-ssh"
if [ ! -f "$marker" ]; then
    /usr/bin/sudo -n /bin/sh -ec '
        config=$(/usr/bin/mktemp /etc/ssh/sshd_lease_config.XXXXXX)
        trap '\''/bin/rm -f -- "$config"'\'' EXIT
        {
            /usr/bin/printf "%s\n" "PasswordAuthentication no" "KbdInteractiveAuthentication no" "PermitRootLogin no"
            /bin/cat /etc/ssh/sshd_config
        } > "$config"
        /usr/sbin/sshd -t -f "$config"
        /bin/chmod 644 "$config"
        /bin/mv "$config" /etc/ssh/sshd_config
        /bin/rm -f -- /etc/ssh/ssh_host_ed25519_key /etc/ssh/ssh_host_ed25519_key.pub
        /usr/bin/ssh-keygen -q -t ed25519 -N "" -f /etc/ssh/ssh_host_ed25519_key
        /usr/sbin/sshd -t
    '
    : > "$marker"
fi
/usr/bin/sudo -n /bin/cat /etc/ssh/ssh_host_ed25519_key.pub
'''


class Worker:
    def __init__(self, settings):
        self.settings = settings
        self.state = Path(settings['state'])
        self.store = Path(settings['tart_home'])
        if any(p.resolve(strict=True) != p or not p.is_dir() for p in (self.state, self.store)):
            raise ValueError('Worker directories must be exact existing absolute paths.')
        self.binary = settings['tart_binary']
        self.template = settings['template']
        self.template_digest = settings['template_config_sha256']
        self.identity = settings['identity']
        self.shape = settings['shape']
        self.network = ipaddress.IPv4Network(settings['guest_network'])
        if (not Path(self.binary).is_absolute() or not re.fullmatch(r'[A-Za-z0-9_-]{1,63}', self.template)
                or self.template.startswith('lease-') or not LEASE.fullmatch(self.identity)
                or not re.fullmatch(r'en[0-9]+', settings['interface'])):
            raise ValueError('Invalid dedicated Tart worker scope')
        self.env = {**os.environ, 'TART_HOME': str(self.store), 'TART_NO_AUTO_PRUNE': '1'}
        self.lock = threading.RLock()
        self.inflight = set()

    def tart(self, *command, timeout=12, data=None):
        return subprocess.check_output([self.binary, *command], input=data, timeout=timeout,
                                       stderr=subprocess.PIPE, env=self.env)

    def guests(self):
        return {v['Name']: v for v in json.loads(self.tart('list', '--format', 'json')) if v['Source']=='local'}

    @staticmethod
    def name(lease):
        if not isinstance(lease, str) or not LEASE.fullmatch(lease):
            raise Rejected('invalid_lease', 'Invalid Apple lease identity.', 400)
        return 'lease-' + lease

    def vm(self, name):
        return self.store / 'vms' / name

    def workspace(self, lease):
        return self.state / (self.name(lease) + '-workspace')

    def journal(self, lease):
        return self.state / (self.name(lease) + '.json')

    def load(self, lease):
        location = self.journal(lease)
        try:
            return json.loads(location.read_text())
        except FileNotFoundError:
            return None

    def save(self, lease, row):
        with tempfile.NamedTemporaryFile(dir=self.state, mode='w', delete=False) as pending:
            json.dump(row, pending)
            pending.flush()
            os.fsync(pending.fileno())
        pending = Path(pending.name)
        try:
            pending.replace(self.journal(lease))
            fd = os.open(self.state, os.O_RDONLY)
            try:
                os.fsync(fd)
            finally:
                os.close(fd)
        finally:
            pending.unlink(missing_ok=True)

    def generation(self, lease):
        root = self.vm(self.name(lease))
        workspace = self.workspace(lease)
        values = []
        for file, mode in ((root, stat.S_IFDIR), (root/'config.json', stat.S_IFREG),
                           (root/'disk.img', stat.S_IFREG), (workspace, stat.S_IFDIR)):
            info = file.lstat()
            if stat.S_IFMT(info.st_mode) != mode:
                raise Rejected('apple_identity_changed', 'The owned guest or workspace path changed.')
            values.extend((info.st_dev, info.st_ino))
        return hashlib.sha256(json.dumps([self.identity, lease, values]).encode()).hexdigest()

    def owned(self, lease, generation=None):
        row = self.load(lease)
        if (not row or row.get('deleted') or not row.get('generation')
                or (generation is not None and generation != row['generation'])
                or self.generation(lease) != row['generation']):
            raise Rejected('apple_identity_changed', 'The Tart guest or workspace does not match the enrolled lease.')
        return row

    def idle(self):
        if any(not json.loads(p.read_text()).get('deleted') for p in self.state.glob('lease-*.json')):
            raise Rejected('apple_slot_occupied', 'The Apple slot still has an owned guest or unresolved operation.')
        if any(re.fullmatch(r'lease-[a-f0-9]{32}', n) for n in self.guests()):
            raise Rejected('apple_slot_occupied', 'A guest using the lease namespace already exists.')
        if any(self.state.glob('lease-*-workspace')):
            raise Rejected('apple_slot_occupied', 'A prior workspace remains in the enrolled Apple scope.')
        return True

    def capacity(self, allow_running):
        guests = self.guests()
        template = self.vm(self.template)/'config.json'
        if (self.template not in guests or guests[self.template]['Running']
                or hashlib.sha256(template.read_bytes()).hexdigest()!=self.template_digest):
            raise Rejected('apple_recipe_changed', 'The pinned and stopped Tart template cannot be verified.')
        running = sum(v['Running'] for v in guests.values())
        if running >= self.settings['max_running'] and not allow_running:
            raise Rejected('apple_capacity_exhausted', 'The Mac already has the maximum verified running guests.')
        sample = subprocess.check_output(['/usr/bin/memory_pressure','-Q'], timeout=4, text=True)
        total = int(re.search(r'has (\d+)', sample)[1])
        percent = int(re.search(r'percentage: (\d+)%', sample)[1])
        return {
            'shape':self.shape,
            'memory_after_start':total*percent//100 - self.shape['memory_mib']*MIB,
            'storage_after_writes':shutil.disk_usage(self.store).free - self.settings['disk_reserve_mib']*MIB,
            'floor_memory_after_start':self.settings['memory_floor_mib']*MIB,
            'floor_storage_after_writes':self.settings['storage_floor_mib']*MIB,
        }

    def tasks(self, operation, lease):
        if operation not in ('clone','start','stop','delete') or (lease is None and operation != 'clone'):
            raise Rejected('apple_operation_identity', 'Invalid operation query.', 400)
        if lease is not None:
            row = self.load(lease)
            return [operation+':'+lease] if row and operation in row['operations'] else []
        records = [json.loads(p.read_text()) for p in self.state.glob('lease-*.json')]
        return [operation+':'+row['lease'] for row in records if not row.get('deleted') and operation in row['operations']]

    def dispatch(self, lease, operation, generation, discard):
        name = self.name(lease)
        if operation not in ('clone','start','stop','delete'):
            raise Rejected('apple_operation_identity', 'Invalid operation.', 400)
        row = self.load(lease)
        if row and operation in row['operations'] and not row.get('deleted'):
            if operation != 'clone':
                self.owned(lease, generation)
            return operation+':'+lease
        if operation == 'clone':
            self.idle()
            result = self.capacity(False)
            if any(result[field] < result['floor_'+field] for field in ('memory_after_start','storage_after_writes')):
                raise Rejected('apple_host_headroom', 'The Apple worker cannot preserve its host headroom.')
            if row or name in self.guests() or self.workspace(lease).exists():
                raise Rejected('apple_identity_changed', 'That Apple lease name has already been used.')
            row = {'lease':lease,'operations':{}}
        else:
            row = self.owned(lease,generation)
            expected = 'clone' if operation=='start' else 'start'
            if row['operations'].get(expected)!='done':
                raise Rejected('apple_operation_state', 'The prior Tart operation is not verified.')
        row['operations'][operation] = 'pending'
        if operation == 'delete' and discard:
            row['discard'] = True
        self.save(lease,row)
        self.inflight.add(operation+':'+lease)
        threading.Thread(target=self.run_operation,args=(lease,operation,row),daemon=True).start()
        return operation+':'+lease

    def run_operation(self, lease, operation, row):
        name = self.name(lease)
        try:
            if operation != 'clone':
                self.owned(lease, row['generation'])
            if operation=='clone':
                self.workspace(lease).mkdir(mode=0o700)
                self.tart('clone',self.template,name,timeout=150)
                row['generation'] = self.generation(lease)
            elif operation=='start':
                capacity = self.capacity(False)
                if any(capacity[field] < capacity['floor_'+field]
                       for field in ('memory_after_start', 'storage_after_writes')):
                    raise Rejected('apple_host_headroom', 'The Apple worker cannot preserve its host headroom.')
                with (self.state/(name+'.log')).open('xb') as log:
                    child=subprocess.Popen([self.binary,'run','--no-graphics','--no-audio','--no-usb-accessories',
                                            '--no-clipboard','--net-bridged='+self.settings['interface'],
                                            '--dir=workspace:'+str(self.workspace(lease)),name],
                                           stdout=log,stderr=log,stdin=subprocess.DEVNULL,env=self.env,
                                           start_new_session=True)
                deadline=time.monotonic()+20
                while not self.guests().get(name,{}).get('Running'):
                    if child.poll() is not None or time.monotonic()>deadline:
                        raise RuntimeError('Tart did not reach running state.')
                    time.sleep(.25)
            elif operation=='stop':
                if self.guests().get(name,{}).get('Running'):
                    self.tart('stop',name,timeout=66)
                if name not in self.guests() or self.guests()[name]['Running']:
                    raise RuntimeError('The owned Tart clone did not stop.')
            elif operation=='delete':
                if (not (row.get('captured') or row.get('discard'))
                        or self.guests().get(name,{}).get('Running')):
                    raise RuntimeError('Tart deletion needs a stopped guest and an explicit workspace disposition.')
                self.tart('delete',name,timeout=45)
                if self.vm(name).exists() or name in self.guests():
                    raise RuntimeError('Tart clone files still exist.')
                shutil.rmtree(self.workspace(lease))
                row['deleted'] = True
            row['operations'][operation]='done'
        except (OSError, ValueError, RuntimeError, Rejected, subprocess.SubprocessError) as error:
            row['operations'][operation]='unknown'
            print(f'{operation} for {lease} held: {type(error).__name__}',flush=True)
        finally:
            with self.lock:
                self.save(lease,row)
                self.inflight.discard(operation+':'+lease)

    def request(self, data):
        action=data['action']
        lease=data.get('lease')
        generation=data.get('generation')
        if action=='idle':
            return self.idle()
        if action=='capacity':
            return self.capacity(data.get('allow_running') is True)
        if action=='tasks':
            return self.tasks(data['operation'],lease)
        if action=='dispatch':
            return self.dispatch(lease,data['operation'],generation,data.get('discard') is True)
        if action=='task':
            match=TASK.fullmatch(data['ref'])
            if not match:
                raise Rejected('apple_operation_identity','Invalid Tart task identifier.',400)
            operation, key=match.groups()
            row=self.load(key)
            state=row and row['operations'].get(operation)
            if state=='done':
                return True
            if state=='pending' and data['ref'] in self.inflight:
                return False
            raise Rejected('apple_operation_unknown','Tart operation is unresolved. The slot stays held.')
        row=self.owned(lease,generation)
        name=self.name(lease)
        if action=='describe':
            return {'generation':row['generation']}
        if action=='status':
            return 'running' if self.guests()[name]['Running'] else 'stopped'
        if action=='address':
            try:
                address=ipaddress.IPv4Address(self.tart('ip',name,'--resolver','agent',timeout=4).decode().strip())
            except (subprocess.SubprocessError,ValueError):
                return None
            if address not in self.network:
                raise Rejected('apple_guest_address_unverified','Tart returned an address outside the enrolled DHCP scope.')
            return str(address)
        if action=='host-key':
            self.guest_user(name)
            try:
                key=self.tart('exec',name,'/bin/sh','-ec',PREPARE_SSH,timeout=7).decode().split()
            except subprocess.SubprocessError as error:
                raise Rejected('apple_guest_key_unverified','The clone could not install an independent SSH host identity.') from error
            key=' '.join(key[:2])
            if not SSH_KEY.fullmatch(key):
                raise Rejected('apple_guest_key_unverified','Invalid Tart guest Ed25519 host key.')
            return key
        if action=='guest-key':
            self.guest_user(name)
            key=data.get('key','')
            if not SSH_KEY.fullmatch(key):
                raise Rejected('invalid_ssh_key','Provide a single Ed25519 task public key.',400)
            self.tart('exec','-i',name,'/bin/sh','-ec',
                      'umask 077; mkdir -p ~/.ssh; cat > ~/.ssh/authorized_keys; test -d "$1"; mkdir -p "$1/project"',
                      'workspace',SHARE,data=(key+'\n').encode(),timeout=8)
            return True
        raise Rejected('apple_operation_identity','Unsupported worker action.',400)

    def guest_user(self,name):
        actual=self.tart('exec',name,'/usr/bin/id','-un',timeout=4).decode().strip()
        if actual!=self.settings['guest_user']:
            raise Rejected('apple_guest_user_changed','The guest agent user differs from the qualified recipe.')

    def capture(self, data, output):
        lease=data['lease']
        row=self.owned(lease,data['generation'])
        if row['operations'].get('start')!='done' or self.guests()[self.name(lease)]['Running']:
            raise Rejected('apple_guest_running','Stop the exact owned clone before archive capture.')
        archive_workspace(self.workspace(lease)/'project',output)
        if self.generation(lease)!=row['generation']:
            raise Rejected('apple_identity_changed','The source changed during capture.')
        row['captured']=True
        self.save(lease,row)


def roles(settings):
    def token(key):
        value=Path(settings[key]).read_text().strip()
        if not re.fullmatch(r'[A-Za-z0-9_-]{43,128}',value):
            raise ValueError('Expected a scoped Tart worker token file.')
        return 'Bearer '+value
    control, capture=token('control_token_file'),token('capture_token_file')
    if control==capture:
        raise ValueError('Capture and control require distinct worker roles.')
    return control, capture


def serve(settings, worker):
    control, capture = roles(settings)

    class Handler(BaseHTTPRequestHandler):
        def setup(self):
            super().setup()
            self.connection.settimeout(12)

        def log_message(self,*args):
            pass

        def answer(self, status, body):
            body=json.dumps(body).encode()
            self.send_response(status)
            self.send_header('Content-Type','application/json')
            self.send_header('Content-Length',str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def do_POST(self):
            if self.path!='/' or self.headers.get('Content-Type')!='application/json':
                self.answer(404,{'code':'not_found','reason':'Worker route unavailable.'})
                return
            try:
                length=int(self.headers.get('Content-Length',0))
                if not 0 < length <= 4096:
                    raise ValueError()
                data=json.loads(self.rfile.read(length))
                want_capture=data['action']=='capture'
                expected=capture if want_capture else control
                supplied=self.headers.get('Authorization','').encode()
                if not hmac.compare_digest(expected.encode(),supplied):
                    self.answer(401,{'code':'unauthorized','reason':'Worker role refused.'})
                    return
                with worker.lock:
                    if want_capture:
                        with tempfile.TemporaryFile(dir=worker.state) as output:
                            worker.capture(data,output)
                            size=output.tell()
                            if not 0 < size <= MAX_ARCHIVE:
                                raise ValueError('Invalid workspace archive size')
                            output.seek(0)
                            self.send_response(200)
                            self.send_header('Content-Type','application/gzip')
                            self.send_header('Content-Length',str(size))
                            self.end_headers()
                            shutil.copyfileobj(output,self.wfile)
                    else:
                        value=worker.request(data)
                        self.answer(200,{'worker':worker.identity,'value':value})
            except Rejected as error:
                self.answer(error.status,{'code':error.code,'reason':error.reason})
            except (OSError, ValueError, TypeError, KeyError, subprocess.SubprocessError) as error:
                print(f'Tart worker request held: {type(error).__name__}',flush=True)
                self.answer(503,{'code':'apple_worker_unverified','reason':'The worker could not verify the request.'})

    ThreadingHTTPServer(('127.0.0.1',settings['port']),Handler).serve_forever()


if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('configuration',type=Path)
    parser.add_argument('--preflight', action='store_true',
                        help='Check the existing pinned template and capacity without starting a worker')
    args = parser.parse_args()
    settings=json.loads(args.configuration.read_text())
    os.umask(0o077)
    worker = Worker(settings)
    if args.preflight:
        roles(settings)
        capacity = worker.capacity(allow_running=False)
        worker.idle()
        print(json.dumps({'template_verified': True, 'slot_idle': True,
                          'shape': capacity['shape'], 'capacity': capacity}))
    else:
        serve(settings,worker)
