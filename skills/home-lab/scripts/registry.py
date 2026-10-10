#!/usr/bin/env python3
"""Home-lab registry client. Credentials are read from a private config, never CLI arguments."""
import argparse, json, os, pathlib, ssl, sys, urllib.request, urllib.error, urllib.parse

def request(config,method,path,body=None,revision=None):
    token=config['reader_token'] if method=='GET' else config['writer_token']
    headers={'Authorization':'Bearer '+token,'Content-Type':'application/json'}
    if revision is not None: headers['If-Match']=str(revision)
    context=ssl.create_default_context(cafile=os.path.expanduser(config['ca_file']))
    req=urllib.request.Request(config['url'].rstrip('/')+path,headers=headers,method=method,data=None if body is None else json.dumps(body).encode())
    with urllib.request.urlopen(req,context=context,timeout=15) as r: return json.load(r)

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
    except (OSError,ValueError,KeyError) as e: print(str(e),file=sys.stderr); sys.exit(1)
