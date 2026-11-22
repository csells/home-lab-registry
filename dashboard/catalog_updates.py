"""Subscribe to committed catalog metadata changes over authenticated HTTPS."""

import json
import logging
import os
import re
import ssl
import time
from http.client import HTTPException
from urllib.error import HTTPError
from urllib.parse import urlsplit
from urllib.request import HTTPSHandler, HTTPRedirectHandler, Request, build_opener


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def consume(stream, received_change, last=None):
    if stream.headers.get_content_type() != 'application/x-ndjson':
        raise ValueError('Invalid catalog stream content type')
    for line in iter(lambda: stream.readline(256), b''):
        if not line.endswith(b'\n'):
            raise ValueError('Invalid catalog stream frame size')
        event = json.loads(line)
        if event == {'heartbeat': True}:
            continue
        if not isinstance(event, dict) or type(event.get('version')) is not int or event['version'] < 0:
            raise ValueError('Invalid catalog stream version')
        epoch = event.get('epoch')
        if epoch is not None and (not isinstance(epoch, str) or not re.fullmatch(r'[0-9a-f]{32}', epoch)):
            raise ValueError('Invalid catalog stream epoch')
        cursor = (epoch, event['version'])
        if cursor != last:
            received_change(cursor)
            last = cursor
    raise ConnectionError('Catalog metadata stream ended')


def subscribe_catalog(config, invalidate):
    origin = urlsplit(config['url'])
    if (origin.scheme != 'https' or not origin.hostname or origin.username
            or origin.password or origin.path not in ('', '/') or origin.query or origin.fragment):
        logging.error('Catalog changes require the registry HTTPS origin')
        return
    context = ssl.create_default_context(cafile=os.path.expanduser(config['ca_file']))
    opener = build_opener(HTTPSHandler(context=context), NoRedirect())
    url = config['url'].rstrip('/') + '/v1/catalog/changes'
    headers = {
        'Authorization': 'Bearer ' + config['reader_token'],
        'Accept': 'application/x-ndjson',
    }
    delay = 2
    last = None

    def received_change(cursor):
        nonlocal delay, last
        delay = 2
        invalidate()
        last = cursor

    while True:
        request = Request(url, headers=headers)
        resume = last if last is not None and last[0] is not None else None
        if resume is not None:
            request.add_header('Last-Event-ID', f'{resume[0]}:{resume[1]}')
        try:
            with opener.open(request, timeout=30) as response:
                consume(response, received_change, resume)
        except HTTPError as error:
            with error:
                if error.code in (401, 403, 404):
                    logging.error('Catalog change subscription unavailable: HTTP %s', error.code)
                    return
                logging.warning('Catalog change subscription will reconnect: HTTP %s', error.code)
        except (HTTPException, OSError, ValueError) as error:
            logging.warning('Catalog change subscription will reconnect: %s', type(error).__name__)
        time.sleep(delay)
        delay = min(delay * 2, 60)
