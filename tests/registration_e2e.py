#!/usr/bin/env python3
"""Run as root ONLY in a disposable Linux VM; exercises real interface changes."""
import base64,contextlib,hashlib,json,os,pathlib,secrets,ssl,subprocess,sys,tempfile,time,urllib.request
ROOT=pathlib.Path(__file__).resolve().parents[1]
sys.path[:0]=[str(ROOT/'dashboard')]
from metrics import connect

def run(*args):return subprocess.check_output(args,stderr=subprocess.STDOUT,text=True)
def note(s):print(s,flush=True)
def wait(fn,label,timeout=70):
    end=time.monotonic()+timeout
    while time.monotonic()<end:
        try:
            value=fn()
            if value:note('PASS '+label);return value
        except (OSError,ValueError,KeyError) as e:
            if 'last_error' not in locals() or str(e)!=last_error:note(label+': '+str(e));last_error=str(e)
        time.sleep(1)
    raise AssertionError(label)

def main():
    if os.getuid()!=0 or os.environ.get('DISPOSABLE_VM_TEST')!='1':raise SystemExit('Requires root and DISPOSABLE_VM_TEST=1 in a disposable VM')
    keep=os.environ.get('REGISTRATION_KEEP_STAGE')=='1'
    if keep:
        state=pathlib.Path('/var/tmp/registration-review-state');state.mkdir(mode=0o700)
        workspace=contextlib.nullcontext(str(state))
    else:workspace=tempfile.TemporaryDirectory(prefix='registration-e2e-',dir='/var/tmp')
    success=False
    with workspace as temp:
        d=pathlib.Path(temp);registry=None;log=None;unit='registration-e2e';authorized=None;old_authorized=None
        def write(name,value):(d/name).write_text(json.dumps(value))
        def cert(name,san,usage):
            run('openssl','req','-new','-newkey','ec','-pkeyopt','ec_paramgen_curve:P-256','-nodes','-subj','/CN='+name,'-keyout',str(d/(name+'.key')),'-out',str(d/(name+'.csr')))
            (d/(name+'.ext')).write_text('subjectAltName='+san+'\nextendedKeyUsage='+usage+'\n')
            run('openssl','x509','-req','-in',str(d/(name+'.csr')),'-CA',str(d/'ca.pem'),'-CAkey',str(d/'ca.key'),'-CAcreateserial','-days','1','-extfile',str(d/(name+'.ext')),'-out',str(d/(name+'.pem')))
        run('openssl','req','-x509','-newkey','ec','-pkeyopt','ec_paramgen_curve:P-256','-nodes','-days','1','-subj','/CN=Disposable-Test-CA','-addext','keyUsage=critical,keyCertSign,cRLSign','-keyout',str(d/'ca.key'),'-out',str(d/'ca.pem'))
        cert('registry','IP:127.0.0.1','serverAuth');cert('node','DNS:test-node','serverAuth');cert('client','DNS:test-client','clientAuth')
        for a,b in [('ca.pem','stream-ca.pem'),('client.pem','stream-client.pem'),('client.key','stream-client-key.pem')]: (d/b).write_bytes((d/a).read_bytes())
        token=secrets.token_urlsafe(32);reader=secrets.token_urlsafe(32);writer=secrets.token_urlsafe(32);(d/'token').write_text(token)
        ctx=ssl.create_default_context(cafile=str(d/'ca.pem'))
        def api(path,body=None,credential=reader,revision=None):
            h={'Authorization':'Bearer '+credential,'Content-Type':'application/json'}
            if revision is not None:h['If-Match']=str(revision)
            method='GET' if body is None else ('PUT' if revision is not None else 'POST')
            req=urllib.request.Request('https://127.0.0.1:9443'+path,headers=h,method=method,data=None if body is None else json.dumps(body).encode())
            with urllib.request.urlopen(req,context=ctx,timeout=7) as r:return json.load(r)
        write('registry.json',{'database':str(d/'catalog.db'),'bind':'127.0.0.1','port':9443,'reader_token':reader,'writer_token':writer,'cert':str(d/'registry.pem'),'key':str(d/'registry.key'),'machine_enrollments':{'machine-test':{'token_sha256':hashlib.sha256(token.encode()).hexdigest(),'allowed_networks':['192.0.2.0/24'],'tls_name':'test-node'}},'presence_tls':{'ca_file':str(d/'ca.pem'),'cert_file':str(d/'client.pem'),'key_file':str(d/'client.key')}})
        try:
            run('ip','link','add','regtest0','type','dummy');run('ip','link','set','regtest0','up');run('ip','addr','add','192.0.2.10/24','dev','regtest0')
            log=(d/'registry.log').open('w')
            registry=subprocess.Popen([sys.executable,str(ROOT/'registry/registry.py'),'--config',str(d/'registry.json')],stdout=log,stderr=log,start_new_session=True)
            wait(lambda:api('/v1/services'),'isolated registry starts')
            spec={'name':'Test node','host':'test-node','owner':'e2e','capabilities':['machine'],'endpoints':{'ssh':'ssh://192.0.2.10'},'lifecycle':'active','runbook':'Disposable test only','dependencies':[],'resources':{'registration':{'endpoint_keys':['ssh'],'metrics':True,'probe':True},'metrics_stream':{'enabled':True,'url':'https://192.0.2.10:9911/stream','server_name':'test-node'}},'probe':{'tcp_host':'192.0.2.10','tcp_port':9911}}
            api('/v1/services/machine-test',spec,writer,0)
            service={**spec,'name':'Test application','capabilities':['test-service'],'endpoints':{'health':'http://192.0.2.10:9911/health','public':'https://example.test'},'resources':{'address_bindings':[{'machine_id':'machine-test','endpoint_keys':['health'],'probe':True}]}}
            api('/v1/services/app',service,writer,0)
            write('agent.json',{'listen':':9911','disks':[{'name':'root','path':'/'}],'cert':str(d/'node.pem'),'key':str(d/'node.key'),'client_ca':str(d/'ca.pem'),'registration':{'url':'https://127.0.0.1:9443/v1/machines/machine-test/presence','token_file':str(d/'token'),'ca_file':str(d/'ca.pem'),'interface':'regtest0'}})
            run('systemd-run','--unit='+unit,'--property=Type=exec',str(ROOT/'agent/lab-metrics'),'-config',str(d/'agent.json'),'-socket',str(d/'agent.sock'))
            def registered(ip):
                r=api('/v1/services/machine-test');return r if r['spec']['resources'].get('agent_presence',{}).get('address')==ip else False
            wait(lambda:registered('192.0.2.10'),'system daemon self-registers without login')
            def stream(ip):
                with connect('https://'+ip+':9911/stream',d,'test-node') as r:return json.loads(r.readline())
            wait(lambda:stream('192.0.2.10').get('history'),'real cached metrics over mTLS')
            try:
                with connect('https://192.0.2.10:9911/stream',d,'wrong-node'):raise AssertionError('Wrong identity accepted')
            except OSError:note('PASS wrong TLS identity rejected')
            # Exercise the real backup transport with an isolated export and key.
            import importlib.util
            module=importlib.util.spec_from_file_location('backup_collect',ROOT/'backup/collect.py');backup=importlib.util.module_from_spec(module);module.loader.exec_module(backup)
            b=d/'backrest';(b/'private').mkdir(parents=True);(b/'deployment').mkdir()
            run('ssh-keygen','-q','-t','ed25519','-N','','-f',str(b/'private/collector-key'))
            hostkey=pathlib.Path('/etc/ssh/ssh_host_ed25519_key.pub').read_text()
            (b/'private/known_hosts').write_text('machine-test '+hostkey)
            (b/'private/registry-reader.json').write_text(json.dumps({'url':'https://127.0.0.1:9443','reader_token':reader,'ca_file':str(d/'ca.pem')}))
            (b/'deployment/manifest.json').write_text(json.dumps({'test':{'machine_id':'machine-test','username':'root'}}))
            (d/'backup-config').write_text('Disposable configuration backup content.\n'*20)
            authorized=pathlib.Path('/root/.ssh/authorized_keys');authorized.parent.mkdir(mode=0o700,exist_ok=True);old_authorized=authorized.read_bytes() if authorized.exists() else None
            public=(b/'private/collector-key.pub').read_text().strip()
            authorized.write_bytes((old_authorized or b'')+('\ncommand="tar -czf - -C '+str(d)+' backup-config",restrict '+public+'\n').encode());authorized.chmod(0o600)
            backup.collect(b,'test');backup.collect(b,'test',True);note('PASS real SSH backup export before IP change')
            note('Changing real interface address .10 -> .11; daemon remains running')
            run('ip','addr','del','192.0.2.10/24','dev','regtest0');run('ip','addr','add','192.0.2.11/24','dev','regtest0')
            r=wait(lambda:registered('192.0.2.11'),'new address discovered automatically')
            assert r['spec']['endpoints']['ssh']=='ssh://192.0.2.11' and r['revision']==1
            s=api('/v1/services/app')['spec'];assert s['endpoints']['health']=='http://192.0.2.11:9911/health' and s['probe']['tcp_host']=='192.0.2.11' and s['endpoints']['public']=='https://example.test'
            note('PASS service links, health target and SSH endpoint follow; public URL unchanged')
            backup.collect(b,'test');note('PASS real SSH backup export follows IP change with unchanged pinned host key')
            assert stream('192.0.2.11')['history'];note('PASS cached stream reconnects with same certificate at new IP')
            run('systemctl','restart',unit);wait(lambda:stream('192.0.2.11').get('history'),'system-service restart recovers independently of login')
            pid=int(run('systemctl','show',unit,'-p','MainPID','--value').strip())
            def usage():
                stat=pathlib.Path(f'/proc/{pid}/stat').read_text().split();mem=pathlib.Path(f'/proc/{pid}/status').read_text();rss=int(next(x.split()[1] for x in mem.splitlines() if x.startswith('VmRSS:')))*1024;return int(stat[13])+int(stat[14]),rss
            a,_=usage();start=time.monotonic();note('Measuring 60 seconds of idle daemon overhead (no viewers)')
            for _ in range(6):time.sleep(10);note('Overhead measurement: '+str(round(time.monotonic()-start))+' seconds')
            b,rss=usage();cpu=(b-a)/os.sysconf('SC_CLK_TCK')/(time.monotonic()-start)*100
            total=int(next(x.split()[1] for x in pathlib.Path('/proc/meminfo').read_text().splitlines() if x.startswith('MemTotal:')))*1024
            note(f'MEASURE CPU one-core={cpu:.4f}% RSS={rss/1024**2:.2f}MiB RAM={rss/total*100:.3f}%')
            assert cpu<1 and rss/total<.01
            note('PASS address-change end-to-end test')
            if keep:
                private=ROOT/'dashboard/private';private.mkdir(mode=0o700,exist_ok=True)
                for name in ['stream-ca.pem','stream-client.pem','stream-client-key.pem']:(private/name).write_bytes((d/name).read_bytes())
                (private/'reader.json').write_text(json.dumps({'url':'https://127.0.0.1:9443','reader_token':reader,'ca_file':str(d/'ca.pem')}))
                (private/'origins.json').write_text('[]')
                (d/'registry.pid').write_text(str(registry.pid))
                launcher=ROOT/'tests/review_server.py'
                run('systemd-run','--unit=registration-review-dashboard',sys.executable,str(launcher))
                note('REVIEW runtime retained on disposable VM; registry PID and state in '+str(d))
            success=True
        finally:
            if not (keep and success):
                subprocess.run(['systemctl','stop',unit],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
                if registry:registry.terminate();registry.wait(timeout=10)
            if log:log.close()
            if authorized is not None:
                if old_authorized is None:authorized.unlink(missing_ok=True)
                else:authorized.write_bytes(old_authorized)
            if not (keep and success):subprocess.run(['ip','link','del','regtest0'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
if __name__=='__main__':main()
