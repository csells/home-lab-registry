"""Read-only inventory comparison. Observations never authorize deletion."""
from datetime import datetime
import math
import time
from model import entity_type
from network_audit import guest_identity

MAX_AGE = 900
GUEST_MISMATCHES = {'missing-guest', 'unregistered-guest'}


def binding(spec):
    resources = spec.get('resources', {})
    explicit = resources.get('runtime_binding', {})
    if not isinstance(explicit, dict): explicit = {}
    if explicit.get('kind') == 'guest':
        return explicit.get('machine_id'), str(explicit['guest_id']) if explicit.get('guest_id') is not None else None
    parent, guest = guest_identity(spec)
    if guest is None:
        deployment = resources.get('deployment', {})
        if isinstance(deployment, dict) and deployment.get('vmid') is not None:
            return 'machine-' + spec.get('host', ''), str(deployment['vmid'])
    return parent, guest

def fresh(sample, now):
    checked, expires = sample.get('checked'), sample.get('expires')
    return (not sample.get('error') and isinstance(sample.get('entries'), list)
            and all(isinstance(v, (int, float)) and math.isfinite(v) for v in (checked, expires))
            and checked <= now + 5 and checked <= expires and now < min(expires, checked + MAX_AGE))

def reconcile(records, observations, now=None, *, lease_claims=None, lease_unverified=()):
    now = time.time() if now is None else now
    active = [r for r in records if r['spec'].get('lifecycle') != 'retired' and entity_type(r) != 'archive']
    hosts = {r['id']: r for r in active if entity_type(r) == 'machine' and not binding(r['spec'])[1]}
    claims, findings, reports, deployments = {}, [], [], []
    lease_claims = lease_claims or {}
    def issue(code, target, message, **fields):
        category = ('guest-mismatch' if code in GUEST_MISMATCHES else
                    'coverage' if code in ('collector-unverified', 'lease-ownership-unverified',
                                           'deployment-unverified') else 'record-review')
        findings.append(dict(code=code, category=category, target=target, message=message, **fields))
    for record in active:
        spec, key = record['spec'], record['id']
        resources = spec.get('resources', {})
        parent, guest = binding(spec)
        if parent and guest:
            claims.setdefault((parent, guest), []).append(record)
        elif entity_type(record) == 'service' and not (isinstance(resources.get('runtime_binding'), dict) and resources['runtime_binding'].get('kind') in ('native', 'external')):
            issue('deployment-unmapped', key, 'Deployment has no structured runtime binding; presence is unverified.')
        runtime = resources.get('runtime_binding')
        if (spec.get('lifecycle') == 'active' and isinstance(runtime, dict)
                and runtime.get('kind') in ('native', 'external')):
            value = record.get('runtime_observation') or {}
            checked, expires = value.get('checked'), value.get('expires')
            observed_state = value.get('state', 'unverified')
            fresh_runtime = (all(type(t) in (int, float) and math.isfinite(t) for t in (checked, expires))
                             and checked <= now + 5 and checked <= expires
                             and now < min(expires, checked + 3600))
            state = observed_state if fresh_runtime and observed_state in ('present', 'missing') else 'unverified'
            deployments.append(dict(record_id=key, kind=runtime['kind'], state=state, checked=checked, expires=expires))
            if state == 'missing':
                issue('deployment-missing', key, 'Enrolled provider reports the bound deployment missing.')
            elif state == 'unverified' and runtime.get('provider') and runtime.get('instance'):
                issue('deployment-unverified', key, 'No fresh verified observation for the bound deployment.')
        if not spec.get('owner'):
            issue('owner-missing', key, 'Record has no owner.')
        expiry = resources.get('expires_at')
        if expiry:
            try:
                date = datetime.fromisoformat(expiry.replace('Z', '+00:00'))
                if date.tzinfo is None: raise ValueError('Timezone required')
                if date.timestamp() < now:
                    issue('expired-lease', key, 'Recorded lease expired; review or renew it. No automatic teardown.', expires_at=expiry)
            except (ValueError, TypeError, AttributeError):
                issue('invalid-lease', key, 'Lease expiry must be an ISO timestamp with timezone.')
    verified = 0
    expected = 0
    for key, host in hosts.items():
        configured = host['spec'].get('resources', {}).get('metrics_stream', {}).get('enabled')
        sample = observations.get(key, {})
        if not configured and not sample:
            reports.append(dict(machine_id=key, status='unconfigured', guests=[], checked=None))
            continue
        expected += 1
        if not fresh(sample, now):
            issue('collector-unverified', key, sample.get('error') or 'Guest report absent, expired or invalid; missing guests cannot be inferred.')
            reports.append(dict(machine_id=key, status='unverified', checked=sample.get('checked'), guests=[]))
            continue
        verified += 1
        entries = sample['entries']
        observed = {g['id']: g for g in entries}
        report = dict(machine_id=key, status='verified', checked=sample['checked'], expires=min(sample['expires'], sample['checked']+MAX_AGE), guests=[])
        templates = host['spec'].get('resources', {}).get('templates', [])
        if isinstance(templates, dict): template_ids = set(templates)
        else: template_ids = {str(t.get('id', t.get('vmid', t.get('name')))) for t in templates}
        for guest in entries:
            linked = claims.get((key, guest['id']), [])
            template = guest.get('template', False) or guest['id'] in template_ids
            lease_key = (key, guest['id'], guest.get('name'), guest.get('type'))
            lease = lease_claims.get(lease_key)
            result = {**guest, 'records': [r['id'] for r in linked], 'template': template}
            if lease:
                result['lease'] = lease
            report['guests'].append(result)
            if not linked and not template and not lease and lease_key in lease_unverified:
                issue('lease-ownership-unverified', key, 'The allocator was unavailable to verify this guest.',
                      guest_id=guest['id'])
            elif not linked and not template and not lease:
                issue('unregistered-guest', key, 'Observed guest has no registered deployment.', guest_id=guest['id'], name=guest.get('name', guest['id']))
        reports.append(report)
        for (parent, gid), linked in claims.items():
            if parent != key: continue
            if gid not in observed:
                for record in linked:
                    issue('missing-guest', record['id'], 'Registered guest absent from a fresh complete host inventory; verify before retirement.', machine_id=key, guest_id=gid)
            machines = [r for r in linked if entity_type(r) == 'machine']
            if len(machines) > 1:
                issue('duplicate-machine', key, 'Multiple machine records claim the same guest.', guest_id=gid, records=[r['id'] for r in machines])
    for (parent, guest), linked in claims.items():
        if parent not in hosts:
            for r in linked: issue('parent-unregistered', r['id'], 'Deployment parent is not an active registered host.', machine_id=parent, guest_id=guest)
    coverage_status = 'unverified' if not verified else 'partial' if verified < expected else 'verified'
    return dict(checked=now, coverage_status=coverage_status,
                status='attention' if findings else coverage_status,
                coverage=dict(verified=verified, expected=expected, unconfigured=len(hosts)-expected),
                findings=findings, hosts=reports, deployments=deployments,
                scope='Deployment presence from guest inventories and enrolled runtime sources. Application health is reported separately.')
