"""Task-side client for the disposable lease pilot."""
import argparse
import hashlib
import ipaddress
import json
import os
import re
import shlex
import ssl
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, fp, code, message, headers, newurl):
        return None


def discover(registry_client, record_id):
    value = json.loads(subprocess.check_output(
        [sys.executable, str(registry_client), 'get', record_id], timeout=15))
    spec = value['spec']
    if spec.get('lifecycle') != 'active' or 'lease-pilot' not in spec.get('capabilities', []):
        raise ValueError('Record does not describe an active lease pilot')
    endpoint = spec.get('endpoints', {}).get('lease_api')
    if not endpoint:
        raise ValueError('No registered lease API endpoint')
    return endpoint


def verified_opener(endpoint, ca):
    parsed = urllib.parse.urlsplit(endpoint)
    if (parsed.scheme != 'https' or not parsed.hostname or parsed.username
            or parsed.password or parsed.path not in ('', '/') or parsed.query or parsed.fragment):
        raise ValueError('Use the registered HTTPS endpoint with an independently obtained CA')
    context = ssl.create_default_context(cafile=str(ca))
    context.minimum_version = ssl.TLSVersion.TLSv1_3
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}),
                                        urllib.request.HTTPSHandler(context=context), NoRedirect())
    return opener


def request(endpoint, ca, token_file, route, payload):
    opener = verified_opener(endpoint, ca)
    token = token_file.read_text().strip()
    if not re.fullmatch(r'[A-Za-z0-9_-]{32,128}', token):
        raise ValueError('Invalid task credential file')
    headers = {'Authorization': 'Bearer ' + token, 'Content-Type': 'application/json'}
    body = None if payload is None else json.dumps(payload).encode()
    with opener.open(urllib.request.Request(endpoint.rstrip('/') + route, data=body,
                                            headers=headers), timeout=20) as response:
        return json.load(response)


def download_checkpoint(grant, ca, destination):
    if not re.fullmatch(r'[a-zA-Z0-9_-]{43}', grant['ticket']):
        raise ValueError('Invalid checkpoint ticket')
    endpoint = grant['endpoint']
    opener = verified_opener(endpoint, ca)
    headers = {'Authorization': 'Bearer ' + grant['ticket']}
    download = urllib.request.Request(endpoint.rstrip('/') + '/v1/checkpoint', headers=headers)
    digest = hashlib.sha256()
    # Publish the verified archive atomically, without replacing an existing file.
    with tempfile.NamedTemporaryFile(dir=destination.parent) as partial:
        with opener.open(download, timeout=150) as response:
            remaining = int(response.headers['Content-Length'])
            if not 0 < remaining <= 1024 ** 3:
                raise ValueError('Invalid archive size')
            while remaining:
                chunk = response.read(min(65536, remaining))
                if not chunk:
                    raise OSError('Checkpoint download ended early')
                digest.update(chunk)
                partial.write(chunk)
                remaining -= len(chunk)
        if digest.hexdigest() != grant['sha256']:
            raise ValueError('Checkpoint digest did not match')
        partial.flush()
        os.link(partial.name, destination)
    return {'path': str(destination), 'sha256': digest.hexdigest()}


def upload_checkpoint(endpoint, ca, token_file, route, path, key):
    with path.open('rb') as archive:
        checksum = hashlib.file_digest(archive, 'sha256').hexdigest()
        size = archive.tell()
        archive.seek(0)
        grant = request(endpoint, ca, token_file, route,
                        {'size': size, 'sha256': checksum, 'request_key': key or 'sha256-' + checksum})
        if 'checkpoint' in grant:
            return grant['checkpoint']
        if not re.fullmatch(r'[a-zA-Z0-9_-]{43}', grant['ticket']):
            raise ValueError('Invalid upload ticket')
        target = grant['endpoint']
        opener = verified_opener(target, ca)
        headers = {'Authorization': 'Bearer ' + grant['ticket'], 'Content-Length': str(size),
                   'Content-Type': 'application/gzip'}
        upload = urllib.request.Request(target.rstrip('/') + '/v1/checkpoint', data=archive, headers=headers)
        with opener.open(upload, timeout=290) as response:
            return json.load(response)


def write_secret(grant, output):
    with tempfile.NamedTemporaryFile(dir=output.parent) as secret:
        secret.write((grant.pop('token') + '\n').encode('ascii'))
        secret.flush()
        os.link(secret.name, output)


def resume_workspace(endpoint, args):
    origin = '/v1/leases/' + args.predecessor
    if not re.fullmatch('[a-f0-9]{32}', args.predecessor):
        raise ValueError('Invalid predecessor lease ID')
    public = subprocess.check_output(['ssh-keygen', '-y', '-f', str(args.identity)], timeout=8,
                                     stdin=subprocess.DEVNULL, text=True).split()
    if len(public) < 2 or public[0] != 'ssh-ed25519':
        raise ValueError('Use an independently generated Ed25519 identity for the replacement')
    def api(path, body=None):
        return request(endpoint, args.ca, args.token_file, path, body)
    payload = {'task': args.task, 'request_key': args.request_key, 'seconds': args.seconds,
               'ssh_public_key': ' '.join(public[:2]), 'reclaim_on_expiry': True,
               'resume_from': args.predecessor, 'checkpoint_id': args.checkpoint_id}
    lease = api('/v1/leases', payload)
    lease_id = lease['id']
    # Emit the operation ID before network-dependent restore steps.
    print('Replacement lease ' + lease_id + '; retry with the same request key after interruption.', file=sys.stderr)
    until = time.monotonic() + 120
    while lease['status'] in ('starting', 'waiting-for-guest-ip') and time.monotonic() < until:
        time.sleep(1)
        lease = api('/v1/leases/' + lease_id)
    if lease['status'] != 'ready':
        raise ValueError('Replacement is ' + lease['status'] + '. Inspect it before retrying.')
    point, ssh = lease['resume_point'], lease['ssh']
    if point['lease'] != args.predecessor:
        raise ValueError('Replacement lineage differs from the requested predecessor')
    result = {'lease': lease_id, 'workspace': '~/workspace', 'recovery_point': point,
              'coverage': ('Declared workspace at final capture.' if point['capture'] == 'final'
                           else 'Declared checkpoint only. Later unacknowledged changes are unavailable.')}
    if lease.get('restore_completed_at'):
        return {**result, 'restore': 'previously-restored', 'acknowledged_at': lease['restore_completed_at']}
    grant = api(origin + '/checkpoint-recovery', {'checkpoint_id': point['checkpoint_id']})
    if point['sha256'] != grant['sha256']:
        raise ValueError('The authorized recovery point changed')
    ip = str(ipaddress.ip_address(ssh['host']))
    if (not re.fullmatch('[a-z_][a-z0-9_-]{0,31}', ssh['user'])
            or not re.fullmatch('[a-zA-Z0-9_-]{1,64}', ssh['host_key_alias'])
            or not re.fullmatch(r'ssh-ed25519 [a-zA-Z0-9+/=]+', ssh['host_key'])):
        raise ValueError('Invalid replacement SSH identity')
    helper = Path(__file__).with_name('restore_workspace.py').read_text()
    with tempfile.TemporaryDirectory() as tmp:
        pin = Path(tmp)/'known_hosts'
        pin.write_text(ssh['host_key_alias'] + ' ' + ssh['host_key'] + '\n')
        archive = Path(tmp)/'checkpoint'
        download_checkpoint(grant, args.ca, archive)
        command = ['ssh', '-T', '-o', 'BatchMode=yes', '-o', 'ConnectTimeout=8',
                   '-o', 'IdentityAgent=none', '-o', 'IdentitiesOnly=yes', '-o', 'ClearAllForwardings=yes',
                   '-o', 'StrictHostKeyChecking=yes', '-o', 'GlobalKnownHostsFile=/dev/null',
                   '-o', 'UserKnownHostsFile='+str(pin), '-o', 'HostKeyAlias='+ssh['host_key_alias'],
                   '-i', str(args.identity), ssh['user']+'@'+ip,
                   shlex.join(['python3','-c',helper,point['sha256'],lease_id])]
        with archive.open('rb') as stream:
            acknowledgment = subprocess.check_output(command, stdin=stream, timeout=180, text=True).strip()
        if acknowledgment not in ('restored','previously-restored'):
            raise ValueError('The guest did not acknowledge the restore')
    api('/v1/leases/' + lease_id + '/restore-complete', {'sha256': point['sha256']})
    return {**result, 'restore': acknowledgment}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument('--endpoint')
    source.add_argument('--record')
    parser.add_argument('--registry-client', type=Path)
    parser.add_argument('--ca', required=True, type=Path)
    parser.add_argument('--token-file', required=True, type=Path)
    sub = parser.add_subparsers(dest='command', required=True)
    sub.add_parser('capabilities')
    resume = sub.add_parser('resume')
    resume.add_argument('predecessor')
    resume.add_argument('--task', required=True)
    resume.add_argument('--request-key', required=True)
    resume.add_argument('--seconds', type=int, required=True)
    resume.add_argument('--identity', required=True, type=Path)
    resume.add_argument('--checkpoint-id')
    acquire = sub.add_parser('acquire')
    acquire.add_argument('--task', required=True)
    acquire.add_argument('--request-key', required=True)
    acquire.add_argument('--seconds', type=int, required=True)
    acquire.add_argument('--ssh-public-key', type=Path, required=True)
    acquire.add_argument('--recipe', default='debian13-amd64')
    acquire.add_argument('--requirements', type=json.loads, help='Required OS, architecture, GPU, vCPUs, memory_mib or disk_mib as JSON')
    acquire.add_argument('--reclaim-on-expiry', action='store_true', help=argparse.SUPPRESS)
    for operation in ('inspect', 'renew', 'release', 'hold', 'assign-reviewer', 'remove-reviewer', 'reviewer-hold', 'override-reviewer', 'retry-export', 'pin', 'unpin', 'recover', 'delegate-renewal', 'revoke-renewal', 'delegated-renew', 'retain-failed-export', 'discard-failed-export', 'checkpoint'):
        command = sub.add_parser(operation)
        command.add_argument('lease')
        if operation == 'renew':
            command.add_argument('--seconds', type=int, required=True)
        elif operation == 'release':
            disposition = command.add_mutually_exclusive_group()
            disposition.add_argument('--discard-disposable-data', action='store_true')
            disposition.add_argument('--preserve-checkpoint', action='store_true')
        elif operation == 'assign-reviewer':
            command.add_argument('--reviewer', required=True)
        elif operation == 'override-reviewer':
            command.add_argument('--confirm-reviewer', required=True)
            command.add_argument('--reason', required=True)
        elif operation in ('hold','reviewer-hold'):
            command.add_argument('--mode', choices=('running', 'disk', 'clear'), default='disk')
            command.add_argument('--reason')
        elif operation == 'delegate-renewal':
            command.add_argument('--duration-seconds', type=int, required=True)
            command.add_argument('--extension-seconds', type=int, required=True)
            command.add_argument('--write-token', type=Path, required=True)
        elif operation == 'checkpoint':
            command.add_argument('--archive', type=Path, required=True)
            command.add_argument('--request-key', help='Defaults to the content digest.')
        elif operation == 'recover':
            command.add_argument('--output', type=Path, required=True)
        elif operation in ('pin', 'retain-failed-export', 'discard-failed-export'):
            command.add_argument('--reason', required=True)
            if operation == 'discard-failed-export':
                command.add_argument('--confirm-lease', required=True)
        if operation in ('recover', 'pin', 'unpin'):
            command.add_argument('--checkpoint-id')
    reserve = sub.add_parser('reserve-capacity')
    reserve.add_argument('--request-key', required=True)
    reserve.add_argument('--purpose', required=True)
    reserve.add_argument('--memory-mib', type=int, default=0)
    reserve.add_argument('--storage-mib', type=int, default=0)
    sub.add_parser('list-reservations')
    release = sub.add_parser('release-reservation')
    release.add_argument('reservation')
    issue = sub.add_parser('issue-task-credential')
    issue.add_argument('--task-ref', required=True)
    issue.add_argument('--seconds', type=int, required=True)
    issue.add_argument('--write-token', type=Path, required=True)
    revoke = sub.add_parser('revoke-task-credential')
    revoke.add_argument('--task-ref', required=True)
    args = parser.parse_args()
    if args.record and not args.registry_client:
        parser.error('--record requires --registry-client')
    try:
        endpoint = args.endpoint or discover(args.registry_client, args.record)
        if args.command == 'resume':
            print(json.dumps(resume_workspace(endpoint, args)))
            return 0
        if args.command == 'capabilities':
            print(json.dumps(request(endpoint, args.ca, args.token_file, '/v1/capabilities', None)))
            return 0
        if args.command in ('issue-task-credential', 'revoke-task-credential'):
            payload = {'task_ref': args.task_ref}
            if args.command == 'issue-task-credential':
                if args.write_token.exists() or args.write_token.is_symlink():
                    raise ValueError('Output credential file already exists')
                payload['seconds'] = args.seconds
            else:
                payload['revoke'] = True
            grant = request(endpoint, args.ca, args.token_file, '/v1/task-credentials', payload)
            if args.command == 'issue-task-credential':
                write_secret(grant, args.write_token)
            print(json.dumps(grant))
            return 0
        if args.command in ('reserve-capacity', 'list-reservations', 'release-reservation'):
            route, payload = '/v1/capacity-reservations', None
            if args.command == 'reserve-capacity':
                payload = {'request_key': args.request_key, 'purpose': args.purpose,
                           'memory_mib': args.memory_mib, 'storage_mib': args.storage_mib}
            elif args.command == 'release-reservation':
                if not re.fullmatch('[a-f0-9]{32}', args.reservation):
                    raise ValueError('Invalid reservation ID')
                route += '/' + args.reservation + '/release'
                payload = {}
            print(json.dumps(request(endpoint, args.ca, args.token_file, route, payload)))
            return 0
        route, payload = '/v1/leases', None
        if args.command == 'acquire':
            key = ' '.join(args.ssh_public_key.read_text().split()[:2])
            payload = {'task': args.task, 'request_key': args.request_key, 'seconds': args.seconds,
                       'recipe': args.recipe, 'ssh_public_key': key}
            if args.reclaim_on_expiry:
                payload['reclaim_on_expiry'] = True
            if args.requirements is not None:
                payload['requirements'] = args.requirements
        else:
            if not re.fullmatch('[a-f0-9]{32}', args.lease):
                raise ValueError('Invalid lease ID')
            route += '/' + args.lease
            if args.command == 'renew':
                route += '/renew'
                payload = {'seconds': args.seconds}
            elif args.command == 'release':
                route += '/release'
                payload = {'discard_disposable_data': args.discard_disposable_data,
                           'preserve_checkpoint': not args.discard_disposable_data}
            elif args.command in ('assign-reviewer', 'remove-reviewer'):
                route += '/reviewer'
                payload = {'reviewer': getattr(args, 'reviewer', None)}
            elif args.command == 'override-reviewer':
                route += '/override-reviewer'
                payload = {'confirm_reviewer': args.confirm_reviewer, 'reason': args.reason}
            elif args.command in ('hold','reviewer-hold'):
                route += '/' + args.command
                payload = {'mode': args.mode, 'reason': args.reason}
            elif args.command in ('retain-failed-export', 'discard-failed-export'):
                route += '/export-disposition'
                payload = {'mode': 'retain' if args.command == 'retain-failed-export' else 'discard',
                           'reason': args.reason, 'confirm_lease': getattr(args, 'confirm_lease', None)}
            elif args.command == 'retry-export':
                route += '/retry-export'
                payload = {}
            elif args.command == 'delegated-renew':
                route = '/v1/delegated-renewal/' + args.lease
                payload = {}
            elif args.command == 'revoke-renewal':
                route += '/delegate-renewal'
                payload = {'revoke': True}
            elif args.command == 'delegate-renewal':
                if args.write_token.exists() or args.write_token.is_symlink():
                    raise ValueError('Output credential file already exists')
                grant = request(endpoint, args.ca, args.token_file, route + '/delegate-renewal',
                                {'duration_seconds': args.duration_seconds,
                                 'extension_seconds': args.extension_seconds})
                write_secret(grant, args.write_token)
                print(json.dumps(grant))
                return 0
            elif args.command == 'checkpoint':
                print(json.dumps(upload_checkpoint(endpoint, args.ca, args.token_file,
                    route + '/checkpoint-upload', args.archive, args.request_key)))
                return 0
            elif args.command == 'recover':
                if args.output.exists() or args.output.is_symlink():
                    raise ValueError('Recovery output already exists')
                grant = request(endpoint, args.ca, args.token_file, route + '/checkpoint-recovery', {'checkpoint_id': args.checkpoint_id})
                print(json.dumps(download_checkpoint(grant, args.ca, args.output)))
                return 0
            elif args.command in ('pin', 'unpin'):
                route += '/checkpoint-pin'
                payload = {'pinned': args.command == 'pin', 'reason': getattr(args, 'reason', None), 'checkpoint_id': args.checkpoint_id}
        print(json.dumps(request(endpoint, args.ca, args.token_file, route, payload)))
        return 0
    except urllib.error.HTTPError as error:
        with error:
            try:
                result = json.load(error)
            except ValueError:
                result = {'code': 'http_error', 'reason': f'HTTP {error.code}'}
        print(json.dumps(result), file=sys.stderr)
        return 2 if 400 <= error.code < 500 else 1
    except (OSError, ValueError, TimeoutError, subprocess.SubprocessError) as error:
        print(json.dumps({'code': 'client_error', 'reason': str(error)}), file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main())
