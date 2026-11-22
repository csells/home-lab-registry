"""Restrict a storage-side Restic command to one allocator repository and role."""
import argparse
import json
import os
import re
import shlex
import subprocess
import sys
from pathlib import Path

TAG = 'lease-final-export'
SNAPSHOT = re.compile(r'[a-f0-9]{64}')
CHALLENGE = re.compile(r'lease-export-[a-f0-9]{32}')
ARCHIVE = re.compile(r'lease-[a-f0-9]{32}(?:-([a-f0-9]{32}))?\.tar\.gz')


class Denied(Exception):
    pass


def dispatch(mode, args, restic):
    if mode not in ('read', 'write', 'expire'):
        raise Denied
    if mode in ('write', 'expire') and (len(args) == 4
            and args[:2] == ['snapshots', '--tag'] and CHALLENGE.fullmatch(args[2]) and args[3] == '--json'):
        return subprocess.run(restic + args).returncode

    if mode == 'write' and len(args) == 9 and args[:3] == ['backup', '--stdin', '--stdin-filename']:
        archive = ARCHIVE.fullmatch(args[3])
        challenge = args[7]
        if (archive and args[4:7] == ['--tag', TAG, '--tag'] and CHALLENGE.fullmatch(challenge)
                and args[8] == '--json'
                and (archive[1] is None or challenge == 'lease-export-' + archive[1])):
            return subprocess.run(restic + args).returncode

    if mode in ('read', 'write') and len(args) == 3 and args[0] == 'dump' and SNAPSHOT.fullmatch(args[1]):
        archive = ARCHIVE.fullmatch(args[2][1:]) if args[2].startswith('/') else None
        if archive:
            verify_managed(restic, args[1], path=args[2])
            return subprocess.run(restic + args).returncode

    if mode == 'expire' and args == ['list', 'snapshots']:
        return subprocess.run(restic + args).returncode

    if mode == 'expire' and len(args) == 3 and args[0] == 'forget' and SNAPSHOT.fullmatch(args[1]) and args[2] == '--json':
        verify_managed(restic, args[1])
        return subprocess.run(restic + args).returncode

    raise Denied


def verify_managed(restic, snapshot, path=None):
    command = restic + ['snapshots', snapshot, '--json']
    rows = json.loads(subprocess.check_output(command, timeout=30))
    if not isinstance(rows, list) or len(rows) != 1:
        raise Denied
    row, = rows
    paths, tags = row['paths'], row['tags']
    if (row['id'] != snapshot or len(paths) != 1 or not paths[0].startswith('/')
            or not ARCHIVE.fullmatch(paths[0][1:]) or TAG not in tags
            or len([tag for tag in tags if CHALLENGE.fullmatch(tag)]) != 1
            or (path is not None and paths != [path])):
        raise Denied


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('config', type=Path)
    parser.add_argument('command', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    config = json.loads(args.config.read_text())
    try:
        command = args.command
        if command == ['--ssh-original-command']:
            original = os.environ.get('SSH_ORIGINAL_COMMAND', '')
            if not 0 < len(original) <= 1024:
                raise Denied
            command = shlex.split(original)
        return dispatch(config['role'], command, config['restic_command'])
    except Denied:
        print('Command is outside the storage credential scope.', file=sys.stderr)
        return 2
    except (OSError, ValueError, TypeError, KeyError, subprocess.SubprocessError) as error:
        print(f'Storage verification failed: {type(error).__name__}', file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main())
