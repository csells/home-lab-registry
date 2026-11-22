"""Read an exact stopped Tart workspace using only the capture role."""
import argparse
import json
import re
import sys
import urllib.request
from pathlib import Path

from restore_workspace import MAX_ARCHIVE
from tart_backend import LEASE, NoRedirect


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('config', type=Path)
    parser.add_argument('slot', choices=['apple-workspace'])
    parser.add_argument('lease')
    parser.add_argument('generation')
    args = parser.parse_args()
    config = json.loads(args.config.read_text())
    url = config['url']
    token = Path(config['token_file']).read_text().strip()
    if (not LEASE.fullmatch(args.lease) or not re.fullmatch(r'[0-9a-f]{64}', args.generation)
            or not re.fullmatch(r'http://127\.0\.0\.1:[1-9][0-9]{0,4}', url)
            or not re.fullmatch(r'[A-Za-z0-9_-]{43,128}', token)):
        raise ValueError('Invalid exact workspace identity or worker route')
    body = json.dumps({'action':'capture','lease':args.lease,'generation':args.generation}).encode()
    request = urllib.request.Request(url, body, headers={'Authorization':'Bearer '+token,
                                                         'Content-Type':'application/json'})
    opener=urllib.request.build_opener(urllib.request.ProxyHandler({}),NoRedirect())
    with opener.open(request,timeout=90) as reply:
        remaining=int(reply.headers['Content-Length'])
        if not 0 < remaining <= MAX_ARCHIVE or reply.headers.get_content_type()!='application/gzip':
            raise ValueError('Tart workspace response exceeded its archive scope')
        while remaining:
            block=reply.read(min(remaining,65536))
            if not block:
                raise ValueError('Tart workspace stream ended early')
            sys.stdout.buffer.write(block)
            remaining-=len(block)


if __name__=='__main__':
    main()
