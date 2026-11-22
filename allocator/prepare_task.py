"""Prepare only the scoped access files a fresh task needs from its launcher."""
import argparse
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

from client import discover, request, write_secret


def prepare(config, task, destination, seconds):
    endpoint = discover(Path(config['registry_client']), config['registry_record'])
    ca = Path(config['ca_file'])
    directory = destination.resolve()
    if directory != destination or not directory.is_absolute():
        raise ValueError('Use an exact absolute path for the new task workspace.')
    directory.mkdir(mode=0o700)
    issued = False
    try:
        (directory/'ca.pem').write_bytes(ca.read_bytes())
        subprocess.run(['ssh-keygen','-q','-t','ed25519','-N','','-f',str(directory/'identity')],
                       check=True,stdin=subprocess.DEVNULL,timeout=12)
        grant = request(endpoint,ca,Path(config['issuer_token_file']),'/v1/task-credentials',
                        {'task_ref':task,'seconds':seconds})
        issued = True
        write_secret(grant,directory/'task.token')
        session = directory/'session.json'
        with session.open('x') as out:
            json.dump({'version':1,'endpoint':endpoint,'expires_at':grant['expires_at']},out)
        return session
    except BaseException:
        try:
            if issued:
                request(endpoint,ca,Path(config['issuer_token_file']),'/v1/task-credentials',
                        {'task_ref':task,'revoke':True})
        finally:
            shutil.rmtree(directory)
        raise


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('config',type=Path)
    parser.add_argument('task')
    parser.add_argument('directory',type=Path)
    parser.add_argument('--seconds',type=int,default=3600)
    args=parser.parse_args()
    os.umask(0o077)
    try:
        manifest=prepare(json.loads(args.config.read_text()),args.task,args.directory,args.seconds)
    except (OSError,ValueError,KeyError,subprocess.SubprocessError) as error:
        print(f'Task launcher could not prepare access: {type(error).__name__}',file=sys.stderr)
        return 1
    print(json.dumps({'session':str(manifest)}))
    return 0


if __name__=='__main__':
    sys.exit(main())
