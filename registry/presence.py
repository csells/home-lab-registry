"""Machine-scoped address announcements, separate from operator-owned specifications."""
import hashlib,hmac,ipaddress,json,re,time
from urllib.parse import urlsplit,urlunsplit


def authenticate(enrollments,machine,authorization):
    enrollment=enrollments.get(machine)
    token=authorization.removeprefix('Bearer ') if authorization.startswith('Bearer ') else ''
    if not enrollment or not token or not hmac.compare_digest(hashlib.sha256(token.encode()).hexdigest(),enrollment.get('token_sha256','')):
        return None
    return enrollment


def validate(body,enrollment):
    if not isinstance(body,dict) or set(body)!={'address','interface'}:
        raise ValueError('Expected address and interface')
    address=ipaddress.ip_address(body['address'])
    if address.version!=4 or address.is_loopback or address.is_link_local or address.is_multicast or address.is_unspecified:
        raise ValueError('Expected routable LAN IPv4 address')
    if not any(address in ipaddress.ip_network(cidr) for cidr in enrollment['allowed_networks']):
        raise ValueError('Address outside enrolled networks')
    interface=body['interface']
    if not isinstance(interface,str) or not 1<=len(interface)<=64 or not all(c.isalnum() or c in '-_.' for c in interface):
        raise ValueError('Invalid interface')
    return {'address':str(address),'interface':interface}


def overlay(record,presence,policy=None):
    if not presence:return record
    spec=record['spec'];resources=spec.setdefault('resources',{})
    state={**presence,'status':'current' if presence['expires']>time.time() else 'stale'}
    resources['agent_presence']=state
    # Retain the last known endpoint after expiry, with explicit stale evidence.
    # A newly reported address is applied only to operator-selected login paths.
    policy=resources.get('registration',{}) if policy is None else policy
    def endpoint(url):
        p=urlsplit(url);authority=presence['address']+(':'+str(p.port) if p.port else '')
        return urlunsplit((p.scheme,authority,p.path,p.query,p.fragment))
    for key in policy.get('endpoint_keys',[]):
        if key in spec['endpoints']:spec['endpoints'][key]=endpoint(spec['endpoints'][key])
    for path in resources.get('management_access',[]):
        if path.get('id') in policy.get('access_ids',[]) and path.get('url'):path['url']=endpoint(path['url'])
    stream=resources.get('metrics_stream',{})
    if policy.get('metrics') and stream.get('url'):
        stream['url']=endpoint(stream['url'])
    if policy.get('probe'):
        probe=spec.get('probe',{})
        if 'tcp_host' in probe:probe['tcp_host']=presence['address']
        if 'url' in probe:probe['url']=endpoint(probe['url'])
    return record


def verifier(config):
    import socket,ssl
    context=ssl.create_default_context(cafile=config['ca_file'])
    context.minimum_version=ssl.TLSVersion.TLSv1_3
    context.load_cert_chain(config['cert_file'],config['key_file'])
    def verify(address,enrollment):
        with socket.create_connection((address,9911),timeout=5) as sock:
            with context.wrap_socket(sock,server_hostname=enrollment['tls_name']) as secure:
                secure.sendall(b'GET /stream HTTP/1.1\r\nHost: metrics\r\nConnection: close\r\n\r\n')
                response=b''
                while b'\r\n' not in response and len(response)<128:
                    chunk=secure.recv(128-len(response))
                    if not chunk:break
                    response+=chunk
                if not response.startswith(b'HTTP/1.1 200 '):raise ValueError('Collector unavailable')
    return verify


def validate_policy(policy,binding=False):
    allowed={'endpoint_keys','access_ids','metrics','probe'}|({'machine_id'} if binding else set())
    if not isinstance(policy,dict) or set(policy)-allowed:raise ValueError('Invalid address policy')
    if binding and (not isinstance(policy.get('machine_id'),str) or not re.fullmatch(r'[a-z][a-z0-9-]{0,63}',policy['machine_id'])):raise ValueError('Invalid machine identity')
    for key in ('endpoint_keys','access_ids'):
        values=policy.get(key,[])
        if not isinstance(values,list) or len(values)>32 or not all(isinstance(v,str) and 0<len(v)<128 for v in values):raise ValueError('Invalid bound endpoint list')
    for key in ('metrics','probe'):
        if key in policy and type(policy[key]) is not bool:raise ValueError('Invalid binding flag')
