"""Declare and match the bounded Linux pool's qualified task requirements."""
import json

from checkpoints import Rejected


RECIPE = 'debian13-amd64'
PLATFORM = {'os': 'linux', 'architecture': 'amd64', 'distribution': 'debian13'}


def requirements(value):
    if not isinstance(value, dict):
        raise Rejected('invalid_requirements', 'Provide a requirements object.', 400)
    for key, actual in value.items():
        if key in ('memory_mib', 'disk_mib', 'vcpus'):
            if type(actual) is not int or actual < 1:
                raise Rejected('invalid_requirements', f'{key} must be a positive integer.', 400)
        elif key in ('gpu', *PLATFORM):
            if not isinstance(actual, str) or not actual:
                raise Rejected('invalid_requirements', f'{key} must be a nonempty string.', 400)
        else:
            raise Rejected('unsupported_capability', f'This pool cannot qualify requirement {key!r}.', 422)
    return json.dumps(value, sort_keys=True, separators=(',', ':'))


def match(requested, capacity):
    offered = {**PLATFORM, **capacity, 'gpu': 'none'}
    for key, wanted in requested.items():
        observed = offered[key]
        if (observed < wanted if isinstance(wanted, int) else observed != wanted):
            raise Rejected('unsupported_capability', f'Requested {key}={wanted}; the qualified slot provides {observed}.', 422)
    return offered
