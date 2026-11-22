"""Read-only impact estimate for a change to an enrolled worker recipe."""

import argparse
import json
from pathlib import Path


CAPACITY = ('memory_mib', 'disk_mib', 'vcpus')
REPLACE = ('image_sha256', 'bootstrap_revision', 'network', 'storage', 'worker')
REBOOT = ('vcpus', 'memory_mib')
RECIPE = set(CAPACITY + REPLACE)


def sizes(values):
    if not isinstance(values, dict) or not all(
            type(values.get(key)) is int and values[key] >= 0 for key in CAPACITY):
        raise ValueError('Use observed nonnegative integer capacity for each resource.')
    return {key: values[key] for key in CAPACITY}


def plan(current, desired, active_leases, spare, *, replacement_slot=None):
    if set(current) != RECIPE or set(desired) != RECIPE:
        raise ValueError('Provide a complete current and desired enrolled recipe.')
    prior = sizes(current)
    requested = sizes(desired)
    if not all(all(values.values()) for values in (prior, requested)) or any(
            not isinstance(recipe[key], str) or not recipe[key]
            for recipe in (current, desired) for key in REPLACE):
        raise ValueError('Use positive resource sizes and complete recipe identities.')
    if not isinstance(active_leases, list) or not all(
            isinstance(row, dict) and isinstance(row.get('owner'), str)
            and isinstance(row.get('id'), str) for row in active_leases):
        raise ValueError('Use the live allocator lease identities and owners.')
    changed = sorted(key for key in RECIPE if current[key] != desired[key])
    if set(changed).intersection(REPLACE) or desired['disk_mib'] < current['disk_mib']:
        action = 'replace'
    elif set(changed).intersection(REBOOT):
        action = 'reboot'
    elif changed:
        action = 'grow-disk'
    else:
        action = 'none'

    required = dict.fromkeys(CAPACITY, 0)
    if action == 'replace':
        required = requested
    elif action != 'none':
        required = {key: max(0, desired[key] - current[key]) for key in CAPACITY}
    available = sizes(spare) if spare is not None else None
    short = {key: max(0, required[key] - available[key])
             for key in CAPACITY if available is not None and required[key] > available[key]}
    if available is None and any(required.values()):
        blocker = 'capacity_unverified'
    elif short:
        blocker = 'overlap_capacity_unavailable' if action == 'replace' else 'capacity_unavailable'
    elif action == 'replace' and replacement_slot is not True:
        blocker = 'replacement_slot_unavailable' if replacement_slot is False else 'replacement_slot_unverified'
    elif active_leases and action in ('replace', 'reboot'):
        blocker = 'active_owners_require_review'
    else:
        blocker = None
    return {
        'action': action,
        'changed': changed,
        'affected_leases': [{'id': row['id'], 'owner': row['owner']}
                            for row in sorted(active_leases, key=lambda row: row['id'])]
                           if action != 'none' else [],
        'additional_capacity': required,
        'missing_capacity': short,
        'blocker': blocker,
        'requires_verified_checkpoint': bool(active_leases and action in ('replace', 'reboot')),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('observation', type=Path, help='Trusted recipes, active lease owners and measured spare capacity')
    data = json.loads(parser.parse_args().observation.read_text())
    result = plan(data['current'], data['desired'], data['active_leases'], data['spare'],
                  replacement_slot=data.get('replacement_slot'))
    print(json.dumps(result, indent=2))


if __name__ == '__main__':
    main()
