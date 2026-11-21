"""Export a stopped pilot guest through scoped capture and backup commands."""
import argparse
import fcntl
import hashlib
import json
import os
import re
import selectors
import subprocess
import sys
import tempfile
import time
import urllib.error
from pathlib import Path

from checkpoints import object_name
from client import request

TAG = 'lease-final-export'
METADATA_LIMIT = 1024 ** 2


class ExportError(Exception):
    pass


def collect(command, output, limit, timeout, source=None):
    deadline = time.monotonic() + timeout
    with subprocess.Popen(command, stdin=source if source is not None else subprocess.DEVNULL,
                          stdout=subprocess.PIPE, stderr=subprocess.DEVNULL) as job:
        try:
            with selectors.DefaultSelector() as selector:
                selector.register(job.stdout, selectors.EVENT_READ)
                size = 0
                while True:
                    remaining = deadline - time.monotonic()
                    if remaining <= 0 or not selector.select(remaining):
                        raise ExportError('Export command timed out.')
                    chunk = os.read(job.stdout.fileno(), 65536)
                    if not chunk:
                        break
                    size += len(chunk)
                    if size > limit:
                        raise ExportError('Export command exceeded its output limit.')
                    output.write(chunk)
            if job.wait(timeout=max(0, deadline - time.monotonic())) != 0:
                raise ExportError('Export command reported failure.')
        finally:
            if job.poll() is None:
                job.kill()
        output.seek(0)


def command_json(command, timeout, source=None):
    with tempfile.TemporaryFile() as output:
        collect(command, output, METADATA_LIMIT, timeout, source)
        return output.read()


def digest(file):
    value = hashlib.file_digest(file, 'sha256').hexdigest()
    file.seek(0)
    return value


def store(config, source, filename, challenge, timeout):
    tag = 'lease-export-' + challenge
    restic = config['restic_command']
    limit = config.get('max_archive_bytes', 64 * 1024 ** 2)
    expected = digest(source)
    snapshots = json.loads(command_json(restic + ['snapshots', '--tag', tag, '--json'], timeout))
    if not isinstance(snapshots, list) or len(snapshots) > 1:
        raise ExportError('The export challenge has ambiguous backup history.')
    if snapshots:
        existing, = snapshots
        if existing['paths'] != ['/' + filename] or not {TAG, tag} <= set(existing['tags']):
            raise ExportError('The tagged snapshot belongs to a different checkpoint.')
        snapshot = existing['id']
    else:
        upload = command_json(restic + ['backup', '--stdin', '--stdin-filename', filename,
                                         '--tag', TAG, '--tag', tag, '--json'], timeout, source)
        summaries = [entry for line in upload.splitlines() if line.strip()
                     for entry in [json.loads(line)] if entry.get('message_type') == 'summary']
        if len(summaries) != 1:
            raise ExportError('Backup did not report one snapshot.')
        snapshot = summaries[0]['snapshot_id']
    if not isinstance(snapshot, str) or not re.fullmatch(r'[a-f0-9]{64}', snapshot):
        raise ExportError('Backup did not return a full snapshot ID.')
    with tempfile.TemporaryFile() as restored:
        collect(restic + ['dump', snapshot, '/' + filename], restored, limit, timeout)
        if digest(restored) != expected:
            raise ExportError('Downloaded checkpoint differs from the source.')
    return {'snapshot': snapshot, 'path': '/' + filename, 'sha256': expected}


def export(config, job):
    lease, challenge, guest = (job[key] for key in ('lease_id', 'challenge', 'guest_id'))
    if (not re.fullmatch(r'[a-f0-9]{32}', lease) or not re.fullmatch(r'[a-f0-9]{32}', challenge)
            or guest != str(config['guest_id'])):
        raise ExportError('Export job does not match the configured slot.')
    limit = config.get('max_archive_bytes', 64 * 1024 ** 2)
    timeout = config.get('command_timeout', 120)
    with tempfile.TemporaryFile() as source:
        capture = config['capture_command'] + [guest, lease]
        collect(capture, source, limit, timeout)
        if not source.read(1):
            raise ExportError('Capture produced no bytes.')
        source.seek(0)
        receipt = store(config, source, object_name(lease), challenge, timeout)
    return {'outcome': 'verified', 'challenge': challenge, 'storage_managed': True, **receipt}


def expire(config, job):
    lease, challenge, snapshot = (job[key] for key in ('lease_id', 'challenge', 'snapshot'))
    checkpoint = job.get('checkpoint_id')
    if (not re.fullmatch(r'[a-f0-9]{32}', lease) or not re.fullmatch(r'[a-f0-9]{32}', challenge)
            or not re.fullmatch(r'[a-f0-9]{64}', snapshot)
            or (checkpoint is not None and checkpoint != challenge)
            or job['path'] != '/' + object_name(lease, checkpoint)):
        raise ExportError('Expiry job has an invalid checkpoint identity.')
    restic = config['restic_command']
    timeout = config.get('command_timeout', 120)
    tag = 'lease-export-' + challenge
    def observed(*filters):
        snapshots = json.loads(command_json(restic + ['snapshots', *filters, '--json'], timeout))
        if not isinstance(snapshots, list) or len(snapshots) > 1:
            raise ExportError('Expiry has ambiguous backup history.')
        return snapshots

    snapshots = observed('--tag', tag)
    if snapshots:
        existing, = snapshots
        if (existing['id'] != snapshot or existing['paths'] != [job['path']]
                or not {TAG, tag} <= set(existing['tags'])):
            raise ExportError('Expiry would affect a different snapshot.')
        command_json(restic + ['forget', snapshot, '--json'], timeout)
    stored_ids = command_json(restic + ['list', 'snapshots'], timeout).decode('ascii').split()
    if any(not re.fullmatch(r'[a-f0-9]{64}', snapshot_id) for snapshot_id in stored_ids):
        raise ExportError('Backup storage did not return a valid snapshot inventory.')
    if snapshot in stored_ids:
        raise ExportError('The expired checkpoint is still visible in storage.')
    return {'lease_id': lease, 'checkpoint_id': checkpoint, 'snapshot': snapshot,
            'claim': job['claim'], 'absent': True}


def process(config):
    def call(route, body=None):
        return request(config['endpoint'], Path(config['ca_file']),
                       Path(config['verifier_token_file']), route, body)

    route = '/v1/final-export'
    job = call(route)['export']
    if job is None:
        if config.get('purge_expired_checkpoints') is True:
            expiry_route = '/v1/checkpoint-expiry'
            expiry = call(expiry_route, {})['expiry']
            if expiry is not None:
                call(expiry_route, expire(config, expiry))
                print('Verified removal of the expired snapshot.')
        return 0
    lease = job['lease_id']
    if not isinstance(lease, str) or not re.fullmatch(r'[a-f0-9]{32}', lease):
        raise ExportError('Invalid lease identifier from the controller.')
    try:
        receipt = export(config, job)
    except (ExportError, OSError, ValueError, TypeError, KeyError, subprocess.TimeoutExpired) as error:
        reason = str(error) if isinstance(error, ExportError) else type(error).__name__
        call(route + '/' + lease, {'outcome': 'failed', 'challenge': job['challenge'],
                                   'reason': reason[:240]})
        print(f'Final export failed: {reason}', file=sys.stderr)
        return 1
    call(route + '/' + lease, receipt)
    print('Verified final checkpoint stored and read back.')
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('config', type=Path)
    config = json.loads(parser.parse_args().config.read_text())
    os.umask(0o077)
    with Path(config['lock_file']).open('a') as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            return 0
        try:
            return process(config)
        except (ExportError, OSError, ValueError, KeyError, urllib.error.URLError) as error:
            print(f'Final export needs retry: {type(error).__name__}', file=sys.stderr)
            return 1


if __name__ == '__main__':
    sys.exit(main())
