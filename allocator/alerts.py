"""Deliver one pending export failure through an operator-configured command."""
import argparse
import fcntl
import json
import os
import re
import subprocess
import sys
import urllib.error
from pathlib import Path

from client import request


def deliver(config):
    def call(payload=None):
        return request(config['endpoint'], Path(config['ca_file']),
                       Path(config['alert_token_file']), '/v1/export-alerts', payload)

    event = call()['alert']
    if event is None:
        return False
    # Never forward the export command's diagnostics, environment or credentials.
    if not re.fullmatch(r'[a-f0-9]{32}', event['id']):
        raise ValueError('Invalid export alert ID')
    envelope = {key: event[key] for key in ('id', 'owner', 'lease_id', 'failed_at')}
    envelope['kind'] = 'lease-export-failed'
    command = config['delivery_command']
    if (not isinstance(command, list) or not command
            or not all(isinstance(arg, str) for arg in command)):
        raise ValueError('Delivery command must be a nonempty argument array')
    subprocess.run(command, input=json.dumps(envelope).encode() + b'\n',
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                   timeout=20, check=True)
    if call({'alert_id': event['id']}) != {'delivered': True}:
        raise ValueError('Controller did not acknowledge alert delivery')
    print(f'Export alert delivered: {event["id"]}', flush=True)
    return True


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
            deliver(config)
        except (KeyError, OSError, ValueError, urllib.error.URLError, subprocess.SubprocessError) as error:
            print(f'Export alert needs retry: {type(error).__name__}', file=sys.stderr)
            return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
