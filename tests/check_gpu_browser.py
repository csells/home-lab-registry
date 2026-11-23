"""Verify dashboard telemetry with controlled samples and browser stream visibility."""
import functools
import json
import os
import pathlib
import threading
import time
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlsplit

from playwright.sync_api import expect, sync_playwright

ROOT = pathlib.Path(__file__).resolve().parents[1]


class Quiet(SimpleHTTPRequestHandler):
    def do_GET(self):
        route = urlsplit(self.path)
        if route.path != "/api/metrics/stream":
            return super().do_GET()
        identity = parse_qs(route.query)["machine"][0]
        sample = self.server.metrics[identity]
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()
        try:
            self.wfile.write(("event: metrics\ndata: " + json.dumps(sample) + "\n\n").encode())
            self.wfile.flush()
            self.server.finished.wait(20)
        except (BrokenPipeError, ConnectionResetError):
            pass

    def log_message(self, *args):
        pass


def sample(now, state, percent):
    value = dict(status='available', checked=now, expires=now+180, cpu_percent=10, memory_used=5,
                 memory_total=10, disks=[dict(name='root', used=1, total=10)])
    if state:
        value['gpu'] = dict(status=state, percent=percent)
    return value


with ThreadingHTTPServer(('127.0.0.1', 0), functools.partial(Quiet, directory=str(ROOT/'dashboard'))) as server:
    server.finished = threading.Event()
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        with sync_playwright() as driver:
            browser = driver.chromium.launch(executable_path=os.environ.get('DASHBOARD_CHROMIUM'))
            try:
                for width in (393, 1440):
                    page = browser.new_page(viewport={'width': width, 'height': 900})
                    page.add_init_script("""
                      window.metricStreams = [];
                      const Native = EventSource;
                      window.EventSource = class extends Native {
                        constructor(url) {
                          super(url);
                          if (url.includes('/metrics/')) {
                            this.signal = {url, closed: false};
                            window.metricStreams.push(this.signal);
                          }
                        }
                        close() {
                          if (this.signal) this.signal.closed = true;
                          super.close();
                        }
                      };
                    """)
                    now = time.time()
                    history = [sample(now-590+i*20, 'available', 80 if i % 2 else 5) for i in range(30)]
                    cases = [('available', 0), ('available', 25), ('not_applicable', None), ('unavailable', None), (None, None)]
                    readings = [dict(sample(now, state, value), history=history) for state, value in cases]
                    records = [dict(id=f'fixture-{i}', name=f'Test host {i}', host=f'test-{i}', kind='machine',
                                    lifecycle='active', capabilities=[], metrics=readings[i]) for i in range(len(cases))]
                    page.route('**/api/inventory', lambda route: route.fulfill(json=dict(machines=records, services=[],
                                                                                      backups=[], fetched_at=now)))
                    page.route('**/api/inventory/stream', lambda route: route.abort())

                    server.metrics = {record['id']: reading for record, reading in zip(records, readings)}
                    server.metrics['fixture-0'] = dict(readings[0], cpu_percent=51)
                    page.goto(f'http://127.0.0.1:{server.server_port}/')
                    panels = page.locator('.metrics-panel')
                    expect(panels).to_have_count(5)
                    expect(panels.first.locator('.cpu .telemetry-value')).to_have_text('51.0%')
                    result = panels.evaluate_all("""panels => panels.map(panel => {
                        const gpu=panel.querySelector('.gpu');
                        const sizes=[...gpu.querySelectorAll('.history-bar')].map(bar=>+bar.getAttribute('height'));
                        return {
                          value:gpu.querySelector('.telemetry-value').textContent,
                          overlay:gpu.querySelector('.telemetry-overlay')?.textContent||null,
                          cells:panel.querySelectorAll('.telemetry-cell').length,
                          spread:Math.max(...sizes)-Math.min(...sizes),
                          fits:panel.scrollWidth<=panel.clientWidth,
                        };
                    })""")
                    assert all(row['cells'] == 4 and row['fits'] for row in result), result
                    assert result[0]['value'] == '0.0%' and result[0]['overlay'] is None, result
                    assert result[1]['value'] == '25.0%' and result[1]['spread'] > 20, result
                    assert result[2]['overlay'] == 'N/A', result
                    assert result[3]['overlay'] == result[4]['overlay'] == 'Unavailable', result
                    page.locator('#services-tab').click()
                    page.wait_for_function("window.metricStreams.length && window.metricStreams.every(stream => stream.closed)")
                    before = page.evaluate('window.metricStreams.length')
                    page.locator('#machines-tab').click()
                    page.wait_for_function("before => window.metricStreams.length > before", arg=before)
                    page.close()
                    print(f'PASS {width}px: history, live reading, GPU states and view-scoped stream lifecycle', flush=True)
            finally:
                browser.close()
    finally:
        server.finished.set()
        server.shutdown()
