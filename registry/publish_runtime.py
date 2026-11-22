#!/usr/bin/env python3
"""Publish an enrolled deployment observation through the registry's HTTPS API."""
import argparse
import json
import os
import re
import ssl
import subprocess
import sys
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

from runtime_presence import TOKEN, validate


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, url):
        return None


def observe_systemd(unit):
    if not re.fullmatch(r'[a-zA-Z0-9_.@:-]{1,180}\.service', unit):
        raise ValueError('Use an exact systemd service unit name')
    try:
        status = subprocess.run(
            ['/usr/bin/systemctl', 'show', '--property=Id,LoadState', '--', unit],
            capture_output=True, text=True, timeout=8)
    except (OSError, subprocess.TimeoutExpired):
        return 'unverified'
    fields = dict(line.split('=', 1) for line in status.stdout.splitlines() if '=' in line)
    if fields.get('Id') != unit:
        return 'unverified'
    if fields.get('LoadState') == 'not-found':
        return 'missing'
    if status.returncode == 0 and fields.get('LoadState') in ('loaded', 'masked'):
        return 'present'
    return 'unverified'


def observe_launchd(domain, label):
    if (not re.fullmatch(r'(?:system|gui/[1-9][0-9]{0,9}|user/[1-9][0-9]{0,9})', domain)
            or not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9_.-]{0,179}', label)):
        raise ValueError('Use an exact launchd domain and service label')
    target = f'{domain}/{label}'
    try:
        status = subprocess.run(['/bin/launchctl', 'print', target],
                                capture_output=True, text=True, timeout=8,
                                env={**os.environ, 'LC_ALL': 'C'})
    except (OSError, subprocess.TimeoutExpired):
        return 'unverified'
    if status.returncode == 113 and 'Could not find service' in status.stderr:
        return 'missing'
    first_line = status.stdout.partition('\n')[0]
    if status.returncode == 0 and first_line == target + ' = {':
        return 'present'
    return 'unverified'


def publish(config, state):
    origin = urllib.parse.urlsplit(config['registry'])
    if (origin.scheme != 'https' or not origin.hostname or origin.path not in ('', '/')
            or origin.username or origin.password or origin.query or origin.fragment):
        raise ValueError('Use the authenticated registry HTTPS origin')
    record = config['record_id']
    if not re.fullmatch(r'[a-z][a-z0-9-]{0,63}', record):
        raise ValueError('Invalid registry record identifier')
    token_file = Path(config['token_file'])
    if token_file.stat().st_mode & 0o077:
        raise ValueError('Publisher token file must be private')
    token = token_file.read_text().strip()
    if not TOKEN.fullmatch(token):
        raise ValueError('Invalid publisher token file')
    body = {'state': state, 'ttl_seconds': config.get('ttl_seconds', 900)}
    validate(body)
    context = ssl.create_default_context(cafile=config['ca_file'])
    opener = urllib.request.build_opener(
        urllib.request.HTTPSHandler(context=context), NoRedirect())
    req = urllib.request.Request(
        config['registry'].rstrip('/') + f'/v1/services/{record}/runtime',
        data=json.dumps(body).encode(), headers={
            'Content-Type': 'application/json', 'Authorization': 'Bearer ' + token,
        })
    with opener.open(req, timeout=12) as response:
        result = json.loads(response.read(4096))
    if result.get('state') != state:
        raise ValueError('Observed state did not match the registry acknowledgment')
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('config', type=Path)
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument('--systemd', action='store_true')
    source.add_argument('--launchd', action='store_true')
    source.add_argument('--state', choices=('present', 'missing', 'unverified'))
    args = parser.parse_args()
    try:
        config = json.loads(args.config.read_text())
        if args.systemd:
            state = observe_systemd(config['unit'])
        elif args.launchd:
            state = observe_launchd(config['domain'], config['label'])
        else:
            state = args.state
        print(json.dumps(publish(config, state)))
    except urllib.error.HTTPError as error:
        with error:
            print(f'Runtime observation rejected: HTTP {error.code}', file=sys.stderr)
        return 1
    except (OSError, ValueError, KeyError) as error:
        print(f'Could not publish runtime observation: {type(error).__name__}', file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
