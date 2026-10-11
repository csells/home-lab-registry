"""Example fixed, bounded local checks. Registry records cannot supply shell commands."""
import json, os, pathlib, socket, subprocess, urllib.request

HOME = pathlib.Path.home()
DOCKER = '/usr/local/bin/docker'
BB = str(HOME/'.bb/runtime/server/node_modules/bb-app/host-daemon/dist/bb')
CHECKS = {'bb', 'entity-social', 'gascity', 'matter-bluetooth'}

def run(args):
    env=dict(os.environ, PATH='/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin')
    return subprocess.run(args, capture_output=True, text=True, timeout=15, check=True, env=env).stdout

def check(name):
    try:
        if name == 'bb':
            data=json.loads(run([BB,'status','--json']))
            good='dataDir' in data and data.get('ok') is not False
            return good, 'BB CLI status responded' if good else 'BB CLI status invalid'
        if name == 'entity-social':
            parts=['frontend','backend','tunnel','caddy','postgres']
            names=['entity-social-production-'+part+'-1' for part in parts]
            output=run([DOCKER,'inspect','--format','{{json .State}}',*names])
            states=[json.loads(line) for line in output.splitlines()]
            good=len(states)==len(parts)
            evidence=[]
            for part,state in zip(parts,states):
                status=state.get('Status','missing')
                health=state.get('Health',{}).get('Status')
                good=good and status=='running' and (health=='healthy' if part in ('backend','postgres') else True)
                evidence.append(part+': '+status+('/'+health if health else ''))
            return bool(good), 'Docker '+', '.join(evidence)
        if name == 'gascity':
            opener=urllib.request.build_opener(urllib.request.ProxyHandler({}))
            with opener.open('http://127.0.0.1:8372/health',timeout=5) as response:
                data=json.load(response)
            startup=data.get('startup',{})
            good=data.get('status')=='ok' and startup.get('ready') is True
            return good, 'GasCity health: status='+str(data.get('status'))+', startup='+str(startup.get('phase'))+', ready='+str(startup.get('ready'))
        if name == 'matter-bluetooth':
            results=[]
            for label,port in [('Matter',5580),('Bluetooth proxy',6053)]:
                try:
                    with socket.create_connection(('127.0.0.1',port),timeout=3): pass
                    results.append((True,label+' TCP reachable'))
                except OSError: results.append((False,label+' TCP unavailable'))
            return all(ok for ok,_ in results), '; '.join(note for _,note in results)+' (connectivity only; device operations not tested)'
        return False, 'Unknown local check'
    except Exception as e:
        # Command stderr or HTTP bodies may contain secrets; never publish them.
        return False, name+' check failed: '+type(e).__name__
