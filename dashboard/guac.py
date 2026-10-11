"""Registry-derived Guacamole sessions; SSH keys never leave the server."""
import base64,hashlib,hmac,json,re,time,threading
from urllib.parse import urlparse,quote,urlencode
from urllib.request import Request,urlopen
from urllib.error import HTTPError
from cryptography.hazmat.primitives.ciphers import Cipher,algorithms,modes
from cryptography.hazmat.primitives.padding import PKCS7
from client import request
LOCK=threading.Lock();CACHE={}
API='http://127.0.0.1:8095/guacamole/api/'

def connections(private,records):
    result={};pins=(private/'guacamole_known_hosts').read_text().splitlines()
    for record in records:
        s=record['spec']
        if s['lifecycle']!='active' or 'machine' not in s['capabilities']:continue
        machine=s['host']
        if not re.fullmatch(r'[a-z][a-z0-9-]{0,48}',machine):continue
        for route in s.get('resources',{}).get('management_access',[]):
            if route['id']!='os' or not (route.get('url') or '').startswith('ssh://'):continue
            address=urlparse(route['url']);hostname=address.hostname;port=address.port or 22
            lookup=route.get('host_key_alias') or (hostname if port==22 else '['+hostname+']:'+str(port))
            pinned=next((line for line in pins if line and lookup in line.split()[0].split(',')),None)
            if not pinned or not route.get('username'):continue
            result[machine]={'protocol':'ssh','parameters':{
                'hostname':hostname,'port':str(port),'username':route['username'],
                'host-key':(hostname if port==22 else '['+hostname+']:'+str(port))+' '+' '.join(pinned.split()[1:3]),
                'private-key':(private/'terminal_key').read_text(),
                'font-size':'18','scrollback':'5000','server-alive-interval':'30',
                'color-scheme':'gray-black','backspace':'127','terminal-type':'xterm-256color',
                'command':'export PATH=/opt/homebrew/bin:/usr/local/bin:$PATH; exec tmux -L lab-terminal new-session -A -s browser'}}
    return result

def signed_data(key,payload):
    raw=json.dumps(payload,separators=(',',':')).encode()
    pad=PKCS7(128).padder();padded=pad.update(hmac.new(key,raw,hashlib.sha256).digest()+raw)+pad.finalize()
    enc=Cipher(algorithms.AES(key),modes.CBC(bytes(16))).encryptor()
    return base64.b64encode(enc.update(padded)+enc.finalize()).decode()

def launch(private,machine):
    if not re.fullmatch(r'[a-z][a-z0-9-]{0,48}',machine):raise ValueError('Unknown machine')
    cfg=json.loads((private/'reader.json').read_text())
    available=connections(private,request(cfg,'GET','/v1/services')['services'])
    if machine not in available:raise ValueError('Unavailable machine')
    username=json.loads((private/'login.json').read_text())['username']
    fingerprint=hashlib.sha256(json.dumps(available,sort_keys=True).encode()).hexdigest()
    with LOCK:
        token=CACHE.get('token') if CACHE.get('fingerprint')==fingerprint and time.time()-CACHE.get('created',0)<2700 else None
        if token:
            try:
                with urlopen(Request(API+'session',headers={'Guacamole-Token':token},method='HEAD'),timeout=10):pass
            except HTTPError:token=None
        if not token:
            key=bytes.fromhex((private/'guacamole-key').read_text().strip())
            payload={'username':username,'expires':int((time.time()+3600)*1000),'connections':available}
            req=Request(API+'tokens',data=urlencode({'data':signed_data(key,payload)}).encode(),headers={'Content-Type':'application/x-www-form-urlencoded'})
            with urlopen(req,timeout=15) as response:token=json.load(response)['authToken']
            CACHE.update(token=token,fingerprint=fingerprint,created=time.time())
    client=base64.b64encode((machine+'\0c\0json').encode()).decode().rstrip('=')
    return '/guacamole/#/client/'+quote(client,safe='')+'?token='+quote(token,safe='')
