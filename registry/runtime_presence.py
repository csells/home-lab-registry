"""Bind authenticated deployment observations to an exact catalog target."""
import hashlib
import hmac
import re

KINDS = frozenset(('native', 'external'))
STATES = frozenset(('present', 'missing', 'unverified'))
TOKEN = re.compile(r'[a-zA-Z0-9_-]{32,128}')
PROVIDER = re.compile(r'[a-z][a-z0-9-]{0,31}')


def identity(binding):
    if not isinstance(binding, dict) or binding.get('kind') not in KINDS:
        return None
    provider = binding.get('provider')
    instance = binding.get('instance')
    if (not isinstance(provider, str) or not PROVIDER.fullmatch(provider)
            or not isinstance(instance, str) or not 1 <= len(instance) <= 192
            or not instance.isascii() or not instance.isprintable()):
        return None
    return binding['kind'], provider, instance


def authenticate(publishers, record, authorization):
    publisher = publishers.get(record)
    if publisher is None or not hmac.compare_digest(
            ('Bearer ' + publisher['token']).encode(), authorization.encode()):
        return None
    return publisher['binding']


def enrollment(entries, reader, writer, machine_enrollments):
    if not isinstance(entries, dict):
        raise ValueError('Expected runtime publisher enrollments by record ID')
    tokens = {reader, writer}
    machine_token_hashes = {entry.get('token_sha256') for entry in machine_enrollments.values()}
    for name, entry in entries.items():
        if (not re.fullmatch(r'[a-z][a-z0-9-]{0,63}', name) or not isinstance(entry, dict)
                or set(entry) != {'token', 'binding'} or not identity(entry['binding'])
                or not isinstance(entry['token'], str) or not TOKEN.fullmatch(entry['token'])
                or entry['token'] in tokens
                or hashlib.sha256(entry['token'].encode()).hexdigest() in machine_token_hashes):
            raise ValueError('Each runtime publisher needs an exact binding and unique token')
        tokens.add(entry['token'])
    return entries


def validate(report):
    if not isinstance(report, dict) or set(report) != {'state', 'ttl_seconds'}:
        raise ValueError('Report must contain a state and TTL')
    ttl = report['ttl_seconds']
    if (not isinstance(report['state'], str) or report['state'] not in STATES
            or type(ttl) is not int or not 60 <= ttl <= 3600):
        raise ValueError('Invalid runtime state or TTL')
    return report['state'], ttl
