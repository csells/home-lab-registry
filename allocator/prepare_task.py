"""Prepare only the scoped access files a fresh task needs from its launcher."""
import argparse
import fcntl
import json
import os
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import time
from pathlib import Path

from client import discover, request, write_secret


def write_manifest(directory, endpoint, expires_at):
    path = directory / 'session.json'
    with path.open('x') as out:
        json.dump({'version': 1, 'endpoint': endpoint, 'expires_at': expires_at}, out)
    return path


def discover_allocator(config):
    return discover(Path(config['registry_client']), config['registry_record'],
                    registry_config=config.get('registry_config'))


def prepare(config, task, destination, seconds):
    endpoint = discover_allocator(config)
    ca = Path(config['ca_file'])
    directory = destination.resolve()
    if directory != destination or not directory.is_absolute():
        raise ValueError('Use an exact absolute path for the new task workspace.')
    if os.path.lexists(directory):
        raise FileExistsError('The task directory already exists.')
    pending = Path(tempfile.mkdtemp(prefix='.task-', dir=directory.parent))
    issued = False
    try:
        (pending / 'ca.pem').write_bytes(ca.read_bytes())
        subprocess.run(['ssh-keygen','-q','-t','ed25519','-N','','-f',str(pending / 'identity')],
                       check=True,stdin=subprocess.DEVNULL,timeout=12)
        grant = request(endpoint,ca,Path(config['issuer_token_file']),'/v1/task-credentials',
                        {'task_ref':task,'seconds':seconds})
        issued = True
        write_secret(grant, pending / 'task.token')
        write_manifest(pending, endpoint, grant['expires_at'])
        pending.rename(directory)
        return directory / 'session.json'
    except BaseException:
        try:
            if issued:
                request(endpoint,ca,Path(config['issuer_token_file']),'/v1/task-credentials',
                        {'task_ref':task,'revoke':True})
        finally:
            shutil.rmtree(pending, ignore_errors=True)
        raise


def session_for_task(config, task, seconds):
    if not re.fullmatch(r'[A-Za-z0-9_-]{1,96}', task):
        raise ValueError('Expected a stable task identity from the launcher.')
    root = Path(config['task_sessions'])
    if not root.is_absolute() or root.resolve(strict=True) != root:
        raise ValueError('Use an existing, exact private session directory.')
    info = root.stat()
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.geteuid() or info.st_mode & 0o077:
        raise ValueError('The session directory must be private to the launch user.')
    # Lock outside the session so a cancelled first attempt cannot leave an
    # unusable lock or force a live task to lose its SSH identity.
    lock_fd = os.open(root / (task + '.lock'), os.O_CREAT | os.O_NOFOLLOW | os.O_WRONLY, 0o600)
    with os.fdopen(lock_fd, 'w') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        directory = root / task
        session = directory / 'session.json'
        if not os.path.lexists(directory):
            return prepare(config, task, directory, seconds)
        info = directory.lstat()
        if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.geteuid() or info.st_mode & 0o077:
            raise ValueError('Invalid task directory.')
        current = json.loads(session.read_text())
        if set(current) != {'version', 'endpoint', 'expires_at'} or current['version'] != 1:
            raise ValueError('Unsupported task session.')
        token = directory / 'task.token'
        if token.is_symlink():
            raise ValueError('The task credential path cannot be a symbolic link.')
        if token.is_file() and current['expires_at'] - time.time() > min(seconds / 4, 300):
            return session

        endpoint = discover_allocator(config)
        ca = Path(config['ca_file'])
        if current['endpoint'] != endpoint or ca.read_bytes() != (directory / 'ca.pem').read_bytes():
            raise ValueError('The registered allocator identity changed for this task.')
        grant = request(endpoint, ca, Path(config['issuer_token_file']), '/v1/task-credentials',
                        {'task_ref': task, 'seconds': seconds})
        with tempfile.TemporaryDirectory(dir=directory) as temp:
            pending = Path(temp)
            write_secret(grant, pending / 'task.token')
            write_manifest(pending, endpoint, grant['expires_at'])
            (pending / 'task.token').replace(token)
            (pending / 'session.json').replace(session)
        return session


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('config',type=Path)
    parser.add_argument('task')
    parser.add_argument('directory',type=Path,nargs='?')
    parser.add_argument('--reuse', action='store_true',
                        help='Use the configured private task directory and preserve an existing identity.')
    parser.add_argument('--seconds',type=int,default=3600)
    args=parser.parse_args()
    os.umask(0o077)
    try:
        config = json.loads(args.config.read_text())
        if args.reuse:
            if args.directory is not None:
                parser.error('--reuse uses the configured task_sessions directory')
            manifest = session_for_task(config, args.task, args.seconds)
        else:
            if args.directory is None:
                parser.error('Specify a directory or --reuse')
            manifest = prepare(config, args.task, args.directory, args.seconds)
    except (OSError,ValueError,KeyError,subprocess.SubprocessError) as error:
        print(f'Task launcher could not prepare access: {type(error).__name__}',file=sys.stderr)
        return 1
    print(json.dumps({'session':str(manifest)}))
    return 0


if __name__=='__main__':
    sys.exit(main())
