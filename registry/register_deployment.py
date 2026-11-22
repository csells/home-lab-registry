#!/usr/bin/env python3
"""Register a deployment's exact identity using the trusted registry client."""
import argparse
import json
import re
import subprocess
import sys
from pathlib import Path

from runtime_presence import identity


def deployment_identity(spec):
    resources = spec['resources']
    binding = resources['runtime_binding']
    generation = resources['deployment_generation']
    owner = spec['owner']
    if (not isinstance(binding, dict) or not isinstance(generation, str) or not re.fullmatch(r'[a-zA-Z0-9_.:-]{1,128}', generation)
            or not isinstance(owner, str) or not owner.strip()):
        raise ValueError('Deployment requires an owner and an immutable generation')
    if binding.get('kind') == 'guest':
        host = binding.get('machine_id')
        guest = binding.get('guest_id')
        if (not isinstance(host, str) or not re.fullmatch(r'machine-[a-z0-9][a-z0-9-]*', host)
                or not isinstance(guest, str) or not re.fullmatch(r'[a-zA-Z0-9_.:-]{1,128}', guest)):
            raise ValueError('Guest deployment requires an exact registered parent and guest ID')
    elif not identity(binding):
        raise ValueError('Native or external deployment requires an exact provider binding')
    return owner, binding, generation


def run_client(client, *args, spec=None):
    content = json.dumps(spec).encode() if spec is not None else None
    result = subprocess.run([sys.executable, str(client), *args], input=content,
                            capture_output=True, timeout=30)
    if result.returncode:
        raise RuntimeError('Registry client rejected the operation. Check the record before retrying.')
    return json.loads(result.stdout)


def register(client, record_id, spec):
    expected = deployment_identity(spec)
    if spec['lifecycle'] != 'active':
        raise ValueError('Register an active deployment')
    binding = expected[1]
    if binding['kind'] == 'guest':
        parent = run_client(client, 'get', binding['machine_id'])['spec']
        if parent['lifecycle'] != 'active' or 'machine' not in parent['capabilities']:
            raise ValueError('Guest parent is not an active registered machine')
    try:
        run_client(client, 'put', record_id, '-', '--revision', '0', spec=spec)
    except (RuntimeError, subprocess.TimeoutExpired) as error:
        # A duplicate retry or interrupted response might leave the write committed.
        try:
            current = run_client(client, 'get', record_id)
        except (RuntimeError, subprocess.TimeoutExpired):
            raise error
        if current['spec']['lifecycle'] != 'active' or deployment_identity(current['spec']) != expected:
            raise ValueError('Record ID belongs to another deployment generation') from error
        return current
    current = run_client(client, 'get', record_id)
    if current['spec']['lifecycle'] != 'active' or deployment_identity(current['spec']) != expected:
        raise ValueError('Registry readback changed during deployment registration')
    return current


def retire(client, record_id, spec):
    expected = deployment_identity(spec)
    current = run_client(client, 'get', record_id)
    if deployment_identity(current['spec']) != expected:
        raise ValueError('The record belongs to another deployment generation')
    if current['spec']['lifecycle'] != 'retired':
        try:
            run_client(client, 'retire', record_id, '--revision', str(current['revision']))
        except (RuntimeError, subprocess.TimeoutExpired) as error:
            # A write can commit before the caller loses its reply. Verify the
            # requested generation before treating this as a successful retire.
            current = run_client(client, 'get', record_id)
            if current['spec']['lifecycle'] != 'retired' or deployment_identity(current['spec']) != expected:
                raise error
            return current
    current = run_client(client, 'get', record_id)
    if deployment_identity(current['spec']) != expected or current['spec']['lifecycle'] != 'retired':
        raise ValueError('Registry retirement was not verified')
    return current


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--registry-client', type=Path, required=True)
    parser.add_argument('action', choices=('register', 'retire'))
    parser.add_argument('record')
    parser.add_argument('manifest', help='Path to the verified deployment manifest, or - for stdin')
    args = parser.parse_args()
    if not re.fullmatch(r'[a-z][a-z0-9-]{0,63}', args.record):
        parser.error('Invalid registry record ID')
    operation = register if args.action == 'register' else retire
    try:
        spec = json.load(sys.stdin) if args.manifest == '-' else json.loads(Path(args.manifest).read_text())
        result = operation(args.registry_client, args.record, spec)
        print(json.dumps({'id': result['id'], 'revision': result['revision'],
                          'lifecycle': result['spec']['lifecycle']}))
    except (ValueError, KeyError, TypeError, RuntimeError, OSError, subprocess.SubprocessError) as error:
        # Manifest fields may contain private deployment details.
        print(f'Deployment registration failed ({type(error).__name__}). Verify identity and catalog state.', file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
