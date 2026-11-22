"""Check a restored Backrest bootstrap from an isolated, private restore root."""
import argparse
import json
from pathlib import Path


BOOTSTRAP_FILES = (
    'etc/systemd/system/backrest.service',
    'opt/backrest/config/config.json',
    'opt/backrest/deployment/RECOVERY.md',
    'opt/backrest/deployment/ansible/central.yml',
    'opt/backrest/deployment/manifest.json',
    'opt/backrest/private/rclone.conf',
)


def private_file(root, relative):
    path = root / relative
    if path.resolve() != path or not path.is_file() or not path.stat().st_size:
        raise ValueError(f'Missing or invalid recovery file: {relative}')
    return path


def index(items, kind):
    if not isinstance(items, list) or not items:
        raise ValueError(f'Missing {kind} declarations')
    result = {}
    for item in items:
        if not isinstance(item, dict) or not isinstance(item.get('id'), str) or not item['id']:
            raise ValueError(f'Invalid {kind} declaration')
        if item['id'] in result:
            raise ValueError(f'Duplicate {kind} identity')
        result[item['id']] = item
    return result


def verify(root):
    root = root.resolve()
    if root == Path('/'):
        raise ValueError('Use an isolated restore directory')
    files = {name: private_file(root, name) for name in BOOTSTRAP_FILES}
    config = json.loads(files['opt/backrest/config/config.json'].read_bytes())
    repositories = index(config.get('repos'), 'repository')
    plans = index(config.get('plans'), 'plan')
    if not all(isinstance(repo.get('uri'), str) and repo['uri'] for repo in repositories.values()):
        raise ValueError('Missing repository location')
    if any(plan.get('repo') not in repositories for plan in plans.values()):
        raise ValueError('A plan points to an unknown repository')
    manifest = json.loads(files['opt/backrest/deployment/manifest.json'].read_bytes())
    if not isinstance(manifest, dict) or not manifest:
        raise ValueError('Missing deployment manifest')
    if 'ExecStart=' not in files['etc/systemd/system/backrest.service'].read_text():
        raise ValueError('Missing Backrest service start command')
    return {'bootstrap_files': len(files), 'repositories': len(repositories), 'plans': len(plans)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('restore_root', type=Path)
    args = parser.parse_args()
    try:
        print(json.dumps(verify(args.restore_root)))
    except (OSError, ValueError, TypeError) as error:
        # No restored configuration values belong in logs.
        parser.exit(1, 'Controller restore verification failed (' + type(error).__name__ + ').\n')


if __name__ == '__main__':
    main()
