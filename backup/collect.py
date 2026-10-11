#!/usr/bin/python3
"""Backrest export hook. Resolve each source afresh; pin SSH identity, not its IP."""
import json,pathlib,subprocess,sys,os,shutil,tempfile
sys.path.insert(0,str(pathlib.Path(__file__).resolve().parents[1]/'registry'))
from resolve import ssh_target

def collect(base,name,clean=False):
    manifest=json.loads((base/'deployment/manifest.json').read_text())
    if name not in manifest:raise ValueError('Unknown backup plan')
    staging=base/'staging'/name
    if clean:
        if staging.exists():shutil.rmtree(staging)
        return
    if staging.exists():raise ValueError('Previous staging exists: inspect failed operation before retry')
    if shutil.disk_usage(base).free<2*1024**3:raise ValueError('Insufficient temporary export space')
    spec=manifest[name]
    config=json.loads((base/'private/registry-reader.json').read_text())
    target=ssh_target(config,spec['machine_id'],spec['username'],spec.get('port'))
    staging.parent.mkdir(mode=0o700,exist_ok=True)
    filename=spec.get('filename','config.tar.gz')
    if pathlib.Path(filename).name!=filename:raise ValueError('Invalid archive name')
    with tempfile.TemporaryDirectory(dir=staging.parent,prefix=name+'-incoming-') as temp:
        archive=pathlib.Path(temp)/filename
        with archive.open('wb') as out:
            subprocess.run(['ssh','-p',str(target['port']),'-i',str(base/'private/collector-key'),'-o','BatchMode=yes','-o','StrictHostKeyChecking=yes','-o','ConnectTimeout=10','-o','HostKeyAlias='+target['host_key_alias'],'-o','UserKnownHostsFile='+str(base/'private/known_hosts'),target['destination'],'export'],stdout=out,check=True,timeout=1800)
        if archive.stat().st_size<100:raise RuntimeError('Export unexpectedly small')
        if archive.name.endswith(('.tar.gz','.tar')):subprocess.run(['tar','-tf',str(archive)],stdout=subprocess.DEVNULL,check=True)
        staging.mkdir(mode=0o700);os.replace(archive,staging/archive.name)
    print(name+': fresh export collected; '+str((staging/filename).stat().st_size)+' bytes',flush=True)

if __name__=='__main__':
    collect(pathlib.Path('/opt/backrest'),sys.argv[1],len(sys.argv)>2 and sys.argv[2]=='clean')
