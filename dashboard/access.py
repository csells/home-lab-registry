"""Public login metadata and private, allowlisted credential lookup."""
import json
import os

def public_access(record):
    return record['spec'].get('resources',{}).get('management_access',[])

def credential(private,machine,login,machines=()):
    # Only explicitly provisioned mappings; never interpret registry paths or client paths.
    target = os.environ.get('LAB_DASHBOARD_ACCESS_TARGET')
    if target is not None:
        allowed = any(record['id'] == machine == target and
                      any(route.get('id') == login and route.get('password_available') is True
                          for route in record.get('access', ())) for record in machines)
        if not allowed:
            return None
    data=json.loads((private/'machine-passwords.json').read_text())
    value=data.get(machine,{}).get(login)
    return value if isinstance(value,str) and value else None

def scoped_records(records):
    """Limit a review installation to the registered disposable login target."""
    target = os.environ.get('LAB_DASHBOARD_ACCESS_TARGET')
    if target is None:
        return records
    result = []
    for record in records:
        spec = record['spec']
        resources = spec.get('resources', {})
        access = resources.get('staging_review', {}).get('access', {}) if record['id'] == target else {}
        scoped = {**resources, 'management_access': access.get('management_access', []),
                  'console_login': None, 'review_terminal_id': access.get('terminal_id')}
        result.append({**record, 'spec': {**spec, 'resources': scoped}})
    return result
