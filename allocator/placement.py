"""Select a Proxmox staging worker from fresh, trusted provider observations."""
import argparse
import json
import math
import re
import sys
import time
from pathlib import Path

from capabilities import match, requirements
from checkpoints import Rejected


LIMIT = 120
RESOURCE_KEYS = ('memory_mib', 'disk_mib', 'vcpus')


def valid_amounts(values, keys, *, positive=False):
    return isinstance(values, dict) and all(
        type(values.get(key)) is int and values[key] >= int(positive) for key in keys)


def plan(request, observations, now=None):
    now = time.time() if now is None else now
    requirements(request)
    if not isinstance(observations, list) or not 0 < len(observations) <= 128:
        raise ValueError('Use 1–128 worker observations')
    placements, rejections, ids = [], [], set()
    for row in observations:
        worker = row['worker']
        if not isinstance(worker, str) or not re.fullmatch(r'machine-[a-z0-9-]+', worker) or worker in ids:
            raise ValueError('Use unique registry machine IDs')
        ids.add(worker)
        checked = row['checked_at']
        if (type(checked) not in (int, float) or not math.isfinite(checked)
                or not now - LIMIT <= checked <= now + 5):
            rejections.append({'worker': worker, 'reason': 'capacity_unverified'})
            continue
        recipe, headroom, minimum = (row[key] for key in ('recipe', 'headroom', 'minimum_headroom'))
        if (not valid_amounts(recipe, RESOURCE_KEYS, positive=True)
                or not valid_amounts(headroom, RESOURCE_KEYS[:2])
                or not valid_amounts(minimum, RESOURCE_KEYS[:2])):
            raise ValueError('Use measured MiB, core counts and configured floors')
        try:
            match(request, recipe)
        except Rejected as error:
            rejections.append({'worker': worker, 'reason': error.code})
            continue
        remaining = {key: headroom[key] - recipe[key] for key in RESOURCE_KEYS[:2]}
        if remaining['memory_mib'] < minimum['memory_mib']:
            rejections.append({'worker': worker, 'reason': 'host_memory_headroom'})
        elif remaining['disk_mib'] < minimum['disk_mib']:
            rejections.append({'worker': worker, 'reason': 'host_storage_headroom'})
        else:
            placements.append({'worker': worker, 'checked_at': checked,
                               'headroom_after_start': remaining})
    winner = min(placements, key=lambda item: (
        item['headroom_after_start']['memory_mib'],
        item['headroom_after_start']['disk_mib'], item['worker']), default=None)
    return {'placement': winner, 'eligible': [v['worker'] for v in placements], 'rejected': rejections}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('input', help='Verified observations JSON file, or - for stdin')
    args = parser.parse_args()
    try:
        data = json.load(sys.stdin) if args.input == '-' else json.loads(Path(args.input).read_text())
        result = plan(data['requirements'], data['observations'])
        print(json.dumps(result))
        return 0 if result['placement'] else 2
    except (KeyError, OSError, ValueError, TypeError, Rejected) as error:
        print(f'Invalid placement input: {type(error).__name__}', file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main())
