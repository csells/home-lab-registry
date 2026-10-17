#!/usr/bin/env python3
"""Home-lab registry client. Credentials are read from a private config, never CLI arguments."""
import argparse, json, os, pathlib, ssl, sys, urllib.request, urllib.error, urllib.parse

class RegistryTransportError(RuntimeError):
    """A complete authenticated registry response could not be obtained."""


def _curl_value(value):
    # curl config quoting; secrets travel on stdin, never argv or a temp file.
    return '"' + str(value).replace('\\', '\\\\').replace('"', '\\"').replace('\n', '\\n').replace('\r', '\\r').replace('\t', '\\t') + '"'


def request(config, method, path, body=None, revision=None):
    import subprocess
    import time
    token = config['reader_token'] if method == 'GET' else config['writer_token']
    url = config['url'].rstrip('/') + path
    if urllib.parse.urlsplit(url).scheme != 'https':
        raise RegistryTransportError('Registry URL must use HTTPS')
    headers = ['Authorization: Bearer ' + token, 'Content-Type: application/json',
               'User-Agent: HomeLabRegistry/1']
    if revision is not None:
        headers.append('If-Match: ' + str(revision))
    options = [('url', url), ('cacert', os.path.expanduser(config['ca_file'])),
               ('request', method)] + [('header', h) for h in headers]
    if body is not None:
        options.append(('data-binary', json.dumps(body)))
    curl_config = ''.join(key + ' = ' + _curl_value(value) + '\n' for key, value in options)
    # Keep the sandbox proxy environment intact. Do not follow redirects with a token.
    command = ['curl', '--disable', '--silent', '--show-error', '--proto', '=https',
               '--connect-timeout', '10', '--max-time', '15', '--config', '-',
               '--write-out', '\n%{http_code}']
    attempts = 3 if method == 'GET' else 1
    for attempt in range(attempts):
        try:
            result = subprocess.run(command, input=curl_config.encode(), capture_output=True, timeout=20)
        except FileNotFoundError:
            raise RegistryTransportError('Registry client requires curl on PATH') from None
        except subprocess.TimeoutExpired:
            raise RegistryTransportError('Registry request timed out; no complete response accepted') from None
        if result.returncode:
            # Retry only reads; a failed write may already have reached the server.
            if result.returncode in (18, 28, 52, 56) and attempt + 1 < attempts:
                print('Registry transport interrupted; retrying read (%d/%d)' % (attempt + 2, attempts), file=sys.stderr)
                time.sleep(0.25 * (attempt + 1))
                continue
            detail = result.stderr.decode(errors='replace').replace(token, '[REDACTED]').strip()
            raise RegistryTransportError('Registry transport failed (curl %d): %s; no partial response accepted' % (result.returncode, detail))
        try:
            data, status = result.stdout.rsplit(b'\n', 1)
            code = int(status)
        except (ValueError, TypeError):
            raise RegistryTransportError('Registry transport returned no HTTP status') from None
        if not 200 <= code < 300:
            # Never echo arbitrary response bodies, which could reflect credentials.
            raise RegistryTransportError('Registry HTTP %d; redirects are not followed; verify endpoint and access permissions' % code)
        try:
            return json.loads(data)
        except (ValueError, UnicodeError):
            raise RegistryTransportError('Registry response was not complete valid JSON') from None


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--config',default=os.environ.get('LAB_REGISTRY_CONFIG','~/.config/home-lab/registry.json'))
    s=p.add_subparsers(dest='command',required=True)
    ls=s.add_parser('list'); ls.add_argument('--capability'); ls.add_argument('--host'); ls.add_argument('--status'); ls.add_argument('--include-retired',action='store_true')
    for cmd in ('get','history'):
        q=s.add_parser(cmd); q.add_argument('id')
    q=s.add_parser('put'); q.add_argument('id'); q.add_argument('file',help='JSON spec file, or - for stdin'); q.add_argument('--revision',required=True,type=int)
    q=s.add_parser('retire'); q.add_argument('id'); q.add_argument('--revision',required=True,type=int)
    q=s.add_parser('observe'); q.add_argument('id'); q.add_argument('--status',choices=['healthy','unhealthy','unknown'],required=True); q.add_argument('--evidence',required=True); q.add_argument('--ttl',type=int,default=300)
    a=p.parse_args(); path=pathlib.Path(a.config).expanduser()
    if path.stat().st_mode & 0o077: p.error('Credential config must have mode 0600')
    c=json.loads(path.read_text()); base='/v1/services'
    if hasattr(a,'id'):
        import re
        if not re.fullmatch(r'[a-z][a-z0-9-]{0,63}',a.id): p.error('Invalid service ID')
        base+='/'+a.id
    if a.command=='list':
        query={k:getattr(a,k) for k in ('capability','host','status') if getattr(a,k)}
        if a.include_retired: query['include_retired']='true'
        result=request(c,'GET',base+'?'+urllib.parse.urlencode(query))
    elif a.command in ('get','history'): result=request(c,'GET',base+('/history' if a.command=='history' else ''))
    elif a.command=='put':
        doc=json.load(sys.stdin) if a.file=='-' else json.loads(pathlib.Path(a.file).read_text())
        result=request(c,'PUT',base,doc,a.revision)
    elif a.command=='retire':
        current=request(c,'GET',base)
        if current['revision']!=a.revision: raise SystemExit('Revision conflict; fetch latest record')
        doc=current['spec']; doc['lifecycle']='retired'; result=request(c,'PUT',base,doc,a.revision)
    else: result=request(c,'POST',base+'/observe',{'status':a.status,'evidence':a.evidence,'ttl_seconds':a.ttl})
    print(json.dumps(result,indent=2))
if __name__=='__main__':
    try: main()
    except urllib.error.HTTPError as e: print('Registry HTTP '+str(e.code)+': '+e.read().decode(),file=sys.stderr); sys.exit(1)
    except (OSError,ValueError,KeyError,RegistryTransportError) as e: print(str(e),file=sys.stderr); sys.exit(1)
