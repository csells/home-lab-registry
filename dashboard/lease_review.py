"""Fetch a page of lease metadata through an operator-pinned read-only endpoint."""
import json
import re
import ssl
import urllib.request
from pathlib import Path
from urllib.parse import urlencode, urlsplit


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def page(config_path, cursor, scope='all'):
    config = json.loads(config_path.read_text())
    origin = urlsplit(config['endpoint'])
    if (origin.scheme != 'https' or not origin.hostname or origin.username is not None
            or origin.password is not None or origin.query or origin.fragment or origin.path not in ('', '/')):
        raise ValueError('Configure the TLS allocator endpoint.')
    if scope not in ('all', 'current', 'released', 'counts'):
        raise ValueError('Unknown lease scope.')
    query = {'scope': scope}
    if cursor is not None:
        if not re.fullmatch(r'[1-9][0-9]{0,17}', cursor):
            raise ValueError('Invalid history cursor.')
        query['before'] = cursor
    path = '/v1/lease-review?' + urlencode(query)
    token = Path(config['token_file']).read_text().strip()
    client = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect(),
        urllib.request.HTTPSHandler(context=ssl.create_default_context(cafile=config['ca_file'])))
    req = urllib.request.Request(origin.geturl().rstrip('/') + path, headers={'Authorization': 'Bearer ' + token})
    with client.open(req, timeout=8) as response:
        raw = response.read(256 * 1024 + 1)
    if len(raw) > 256 * 1024:
        raise ValueError('Lease review page exceeded the response budget.')
    return json.loads(raw)
