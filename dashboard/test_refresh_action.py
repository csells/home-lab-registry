import json
import tempfile
import time
import unittest
from pathlib import Path
from threading import Event, Thread
from urllib.request import Request, urlopen
from unittest.mock import patch

import server


class RefreshActionTests(unittest.TestCase):
    def test_refresh_returns_the_authoritative_snapshot_without_browser_sse(self):
        origin = 'https://dashboard.invalid'
        with tempfile.TemporaryDirectory() as directory:
            private = Path(directory)
            (private/'origins.json').write_text(json.dumps([origin]))
            wake = Event()
            pending = Event()
            unblock = Event()
            old = {'catalog':{'checked':1},'fetched_at':1,'error':None}
            fresh = {**old,'catalog':{'checked':time.time()+60}}
            with (patch.object(server,'PRIVATE',private),
                  patch.object(server,'WAKE',wake),
                  patch.object(server,'STATE',old),
                  server.ThreadingHTTPServer(('127.0.0.1', 0),server.Handler) as dashboard):
                dashboard.auth = 'Basic test'
                serving = Thread(target=dashboard.serve_forever,daemon=True)
                serving.start()
                def publish():
                    if wake.wait(3):
                        pending.set()
                        if unblock.wait(3):
                            with server.LOCK:
                                server.STATE=fresh
                            with server.UPDATES:
                                server.UPDATES.notify_all()
                worker = Thread(target=publish,daemon=True)
                worker.start()
                try:
                    req = Request(f'http://127.0.0.1:{dashboard.server_port}/api/refresh',
                                  b'{}', {'Authorization':'Basic test','Origin':origin,
                                          'Content-Type':'application/json','X-Lab-Action':'refresh-catalog'})
                    from concurrent.futures import ThreadPoolExecutor
                    with ThreadPoolExecutor(1) as executor:
                        def refresh():
                            with urlopen(req, timeout=4) as result:
                                return result.status, json.load(result)
                        response = executor.submit(refresh)
                        self.assertTrue(pending.wait(2))
                        unblock.set()
                        code, body = response.result(timeout=4)
                        self.assertEqual(code,200)
                        self.assertEqual(body['catalog'],fresh['catalog'])
                finally:
                    unblock.set()
                    dashboard.shutdown()
                    worker.join(4)
                    serving.join(2)
