"""Subscribe to committed catalog metadata changes over authenticated HTTPS."""

import json
import logging
import os
import re
import ssl
from http.client import HTTPException
from threading import Event
from urllib.error import HTTPError
from urllib.parse import urlsplit
from urllib.request import HTTPSHandler, HTTPRedirectHandler, Request, build_opener


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def consume(stream, received_change, last=None, on_frame=lambda: None, on_health=None):
    if stream.headers.get_content_type() != 'application/x-ndjson':
        raise ValueError('Invalid catalog stream content type')
    for line in iter(lambda: stream.readline(10000 if on_health else 256), b''):
        if not line.endswith(b'\n'):
            raise ValueError('Invalid catalog stream frame size')
        event = json.loads(line)
        if event == {'heartbeat': True}:
            on_frame()
            continue
        if not isinstance(event, dict) or type(event.get('version')) is not int or event['version'] < 0:
            raise ValueError('Invalid catalog stream version')
        epoch = event.get('epoch')
        if epoch is not None and (not isinstance(epoch, str) or not re.fullmatch(r'[0-9a-f]{32}', epoch)):
            raise ValueError('Invalid catalog stream epoch')
        health = event.get('health')
        if health is not None and (not isinstance(health, list) or not 0 < len(health) <= 128
                                   or not all(isinstance(key, str) and re.fullmatch(r'[a-z][a-z0-9-]{0,63}', key) for key in health)):
            raise ValueError('Invalid registry health event')
        on_frame()
        cursor = (epoch, event['version'])
        if cursor != last:
            if health and on_health:
                on_health(cursor, health)
            else:
                received_change(cursor)
            last = cursor
    raise ConnectionError('Catalog metadata stream ended')


def subscribe_catalog(config, invalidate, report_state, stop=None, update_health=None):
    stop = stop or Event()
    origin = urlsplit(config['url'])
    if (origin.scheme != 'https' or not origin.hostname or origin.username
            or origin.password or origin.path not in ('', '/') or origin.query or origin.fragment):
        logging.error('Catalog changes require the registry HTTPS origin')
        report_state(False)
        return
    url = config['url'].rstrip('/') + '/v1/catalog/changes'
    if update_health:
        url += '?health=1'
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

    def received_health(cursor, ids):
        nonlocal last
        update_health(ids)
        last = cursor

    connected = None

    def state(value):
        nonlocal connected
        if connected != value:
            connected = value
            report_state(value)

    def on_frame():
        nonlocal delay
        delay = 2
        state(True)

    while not stop.is_set():
        request = Request(url, headers=headers)
        resume = last if last is not None and last[0] is not None else None
        if resume is not None:
            request.add_header('Last-Event-ID', f'{resume[0]}:{resume[1]}')
        try:
            context = ssl.create_default_context(cafile=os.path.expanduser(config['ca_file']))
            opener = build_opener(HTTPSHandler(context=context), NoRedirect())
            with opener.open(request, timeout=30) as response:
                consume(response, received_change, resume, on_frame, received_health if update_health else None)
        except HTTPError as error:
            with error:
                logging.warning('Catalog change subscription will reconnect: HTTP %s', error.code)
        except (HTTPException, OSError, ValueError) as error:
            logging.warning('Catalog change subscription will reconnect: %s', type(error).__name__)
        if stop.is_set():
            return
        state(False)
        stop.wait(delay)
        delay = min(delay * 2, 60)
