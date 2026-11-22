"""Declare and match the bounded Linux pool's qualified task requirements."""
import json
import re
from urllib.parse import urlsplit

from checkpoints import Rejected


RECIPE = 'debian13-amd64'
PLATFORM = {'os': 'linux', 'architecture': 'amd64', 'distribution': 'debian13'}
DEPENDENCY = re.compile(r'[a-z][a-z0-9-]{0,63}')
SIZE_REQUIREMENTS = ('memory_mib', 'disk_mib', 'vcpus')
MAX_REQUEST_TEXT = 128
MAX_EXTENSION_SECONDS = 86400 * 365
TASK_API_VERSION = 1


def validate_bindings(bindings):
    if not isinstance(bindings, dict) or len(bindings) > 32:
        raise ValueError('Configure up to 32 named shared dependencies')
    for name, entry in bindings.items():
        if (not isinstance(name, str) or not DEPENDENCY.fullmatch(name)
                or not isinstance(entry, dict) or entry.keys() != {'service_id', 'endpoint'}
                or not isinstance(entry['service_id'], str) or not DEPENDENCY.fullmatch(entry['service_id'])
                or not isinstance(entry['endpoint'], str) or not entry['endpoint'].isascii()
                or any(character.isspace() or ord(character) < 32 for character in entry['endpoint'])):
            raise ValueError('Use each qualified dependency name, registry service ID and endpoint')
        url = urlsplit(entry['endpoint'])
        if (url.scheme != 'https' or not url.hostname or url.username is not None
                or url.password is not None or url.query or url.fragment):
            raise ValueError('Shared dependencies need an HTTPS endpoint without inline credentials')
    return bindings


def requirements(value):
    if not isinstance(value, dict):
        raise Rejected('invalid_requirements', 'Provide a requirements object.', 400)
    for key, actual in value.items():
        if key in SIZE_REQUIREMENTS:
            if type(actual) is not int or actual < 1:
                raise Rejected('invalid_requirements', f'{key} must be a positive integer.', 400)
        elif key == 'dependencies':
            if (not isinstance(actual, list) or len(actual) > 16
                    or any(not isinstance(name, str) or not DEPENDENCY.fullmatch(name) for name in actual)
                    or len(set(actual)) != len(actual)):
                raise Rejected('invalid_requirements', 'Provide up to 16 distinct shared dependency names.', 400)
        elif key in ('gpu', *PLATFORM):
            if not isinstance(actual, str) or not actual:
                raise Rejected('invalid_requirements', f'{key} must be a nonempty string.', 400)
        else:
            raise Rejected('unsupported_capability', f'This pool cannot qualify requirement {key!r}.', 422)
    canonical = dict(value)
    if 'dependencies' in canonical:
        canonical['dependencies'] = sorted(canonical['dependencies'])
    return json.dumps(canonical, sort_keys=True, separators=(',', ':'))


def match(requested, capacity, dependencies=(), *, platform=PLATFORM):
    offered = {**platform, **capacity, 'gpu': 'none'}
    for key, wanted in requested.items():
        if key == 'dependencies':
            unavailable = sorted(set(wanted).difference(dependencies))
            if unavailable:
                raise Rejected('dependency_unavailable', 'Unavailable shared dependencies: ' + ', '.join(unavailable), 422)
            continue
        observed = offered[key]
        if (observed < wanted if isinstance(wanted, int) else observed != wanted):
            raise Rejected('unsupported_capability', f'Requested {key}={wanted}; the qualified slot provides {observed}.', 422)
    return offered


def task_contract():
    """Describe the core task routes without exposing credentials or host topology."""
    short_text = {'type': 'string', 'minLength': 1, 'maxLength': MAX_REQUEST_TEXT}
    positive_size = {'type': 'integer', 'minimum': 1}
    duration = {**positive_size, 'maximum': MAX_EXTENSION_SECONDS}
    lease_id = {'type': 'string', 'pattern': '^[a-f0-9]{32}$'}
    properties = {
        **{key: positive_size for key in SIZE_REQUIREMENTS},
        **{key: {'type': 'string', 'minLength': 1} for key in ('gpu', *PLATFORM)},
        'dependencies': {'type': 'array', 'uniqueItems': True, 'maxItems': 16,
                         'items': {'type': 'string', 'pattern': f'^{DEPENDENCY.pattern}$'}},
    }
    return {
        'api_version': TASK_API_VERSION,
        'schema_dialect': 'https://json-schema.org/draft/2020-12/schema',
        'capabilities': '/v1/capabilities',
        'operations': {
            'acquire': {'method': 'POST', 'path': '/v1/leases', 'body_schema': 'acquire'},
            'list': {'method': 'GET', 'path': '/v1/leases'},
            'inspect': {'method': 'GET', 'path': '/v1/leases/{id}'},
            'renew': {'method': 'POST', 'path': '/v1/leases/{id}/renew', 'body_schema': 'renew'},
            'release': {'method': 'POST', 'path': '/v1/leases/{id}/release', 'body_schema': 'release'},
        },
        'schemas': {
            'acquire': {
                'type': 'object',
                'required': ['task', 'request_key', 'seconds', 'ssh_public_key'],
                'properties': {
                    'task': short_text, 'request_key': short_text, 'seconds': duration,
                    'ssh_public_key': {'type': 'string', 'minLength': 1},
                    'recipe': {'type': 'string', 'minLength': 1},
                    'requirements': {'type': 'object', 'additionalProperties': False,
                                     'properties': properties},
                    'resume_from': lease_id, 'checkpoint_id': lease_id,
                    'reclaim_on_expiry': {'const': True},
                },
                'dependentRequired': {'checkpoint_id': ['resume_from']},
            },
            'renew': {'type': 'object', 'required': ['seconds'], 'properties': {'seconds': duration}},
            'release': {'type': 'object', 'properties': {'preserve_checkpoint': {'const': True}}},
        },
    }
