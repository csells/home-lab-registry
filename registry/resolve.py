"""Resolve an enrolled machine for unattended clients without trusting its old IP."""
import re
from urllib.parse import urlsplit
from client import request

def ssh_target(config,machine_id,username,port=None):
    if not re.fullmatch(r'[a-z][a-z0-9-]{0,63}',machine_id):raise ValueError('Invalid machine identity')
    if not re.fullmatch(r'[A-Za-z_][A-Za-z0-9_-]*',username):raise ValueError('Invalid SSH user')
    record=request(config,'GET','/v1/services/'+machine_id)
    spec=record['spec'];presence=spec.get('resources',{}).get('agent_presence',{})
    if spec['lifecycle']!='active' or presence.get('status')!='current':raise ValueError('Machine registration unavailable or stale')
    endpoint=urlsplit(spec['endpoints']['ssh'])
    if endpoint.scheme!='ssh' or not endpoint.hostname or endpoint.username or endpoint.password:raise ValueError('Invalid registered SSH endpoint')
    selected=port if port is not None else (endpoint.port or 22)
    if type(selected) is not int or not 1<=selected<=65535:raise ValueError('Invalid SSH port')
    return {'destination':username+'@'+endpoint.hostname,'port':selected,'host_key_alias':machine_id}
