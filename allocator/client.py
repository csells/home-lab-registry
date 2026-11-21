"""Task-side client for the disposable lease pilot."""
import argparse
import json
import re
import ssl
import subprocess
import sys
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


def request(endpoint, ca, token_file, route, payload):
    parsed = urllib.parse.urlsplit(endpoint)
    if (parsed.scheme != 'https' or not parsed.hostname or parsed.username
            or parsed.password or parsed.path not in ('', '/') or parsed.query or parsed.fragment):
        raise ValueError('Use the registered HTTPS endpoint with an independently obtained CA')
    context = ssl.create_default_context(cafile=str(ca))
    context.minimum_version = ssl.TLSVersion.TLSv1_3
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}),
                                        urllib.request.HTTPSHandler(context=context), NoRedirect())
    token = token_file.read_text().strip()
    if not re.fullmatch(r'[A-Za-z0-9_-]{32,128}', token):
        raise ValueError('Invalid task credential file')
    headers = {'Authorization': 'Bearer ' + token, 'Content-Type': 'application/json'}
    body = None if payload is None else json.dumps(payload).encode()
    with opener.open(urllib.request.Request(endpoint.rstrip('/') + route, data=body,
                                            headers=headers), timeout=20) as response:
        return json.load(response)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument('--endpoint')
    source.add_argument('--record')
    parser.add_argument('--registry-client', type=Path)
    parser.add_argument('--ca', required=True, type=Path)
    parser.add_argument('--token-file', required=True, type=Path)
    sub = parser.add_subparsers(dest='command', required=True)
    acquire = sub.add_parser('acquire')
    acquire.add_argument('--task', required=True)
    acquire.add_argument('--request-key', required=True)
    acquire.add_argument('--seconds', type=int, required=True)
    acquire.add_argument('--ssh-public-key', type=Path, required=True)
    acquire.add_argument('--recipe', default='debian13-amd64')
    for operation in ('inspect', 'renew', 'release'):
        command = sub.add_parser(operation)
        command.add_argument('lease')
        if operation == 'renew':
            command.add_argument('--seconds', type=int, required=True)
        elif operation == 'release':
            command.add_argument('--discard-disposable-data', action='store_true', required=True)
    args = parser.parse_args()
    if args.record and not args.registry_client:
        parser.error('--record requires --registry-client')
    try:
        endpoint = args.endpoint or discover(args.registry_client, args.record)
        route, payload = '/v1/leases', None
        if args.command == 'acquire':
            key = ' '.join(args.ssh_public_key.read_text().split()[:2])
            payload = {'task': args.task, 'request_key': args.request_key, 'seconds': args.seconds,
                       'recipe': args.recipe, 'ssh_public_key': key}
        else:
            if not re.fullmatch('[a-f0-9]{32}', args.lease):
                raise ValueError('Invalid lease ID')
            route += '/' + args.lease
            if args.command == 'renew':
                route += '/renew'
                payload = {'seconds': args.seconds}
            elif args.command == 'release':
                route += '/release'
                payload = {'discard_disposable_data': True}
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
