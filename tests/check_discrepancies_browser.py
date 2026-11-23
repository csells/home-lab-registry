"""Exercise the built Discrepancies UI with controlled responses and no browser SSE."""
import functools
import os
import pathlib
import threading
import time
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from playwright.sync_api import expect, sync_playwright

ROOT = pathlib.Path(__file__).resolve().parents[1]


class Quiet(SimpleHTTPRequestHandler):
    def log_message(self, *args):
        pass


with ThreadingHTTPServer(('127.0.0.1', 0), functools.partial(Quiet, directory=str(ROOT/'dashboard'))) as server:
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        with sync_playwright() as driver:
            browser = driver.chromium.launch(executable_path=os.environ.get('DASHBOARD_CHROMIUM'))
            try:
                for width in (390, 1280):
                    page = browser.new_page(viewport={'width':width,'height':850})
                    errors = []
                    page.on('pageerror', lambda error: errors.append(str(error)))
                    stamp = time.time()
                    accuracy = {'checked':stamp,'hosts':[],'findings':[],
                                'coverage':{'verified':2,'expected':2,'unconfigured':0}}
                    snapshot = {'fetched_at':stamp,'server_time':stamp,'machines':[],'backups':[],
                                'services':[{'id':'web','name':'Web','host':'test','lifecycle':'active',
                                             'capabilities':[],'health':{'status':'unhealthy','expires':stamp+60}}],
                                'inventory_accuracy':accuracy}
                    page.route('**/api/inventory',lambda route:route.fulfill(json=snapshot))
                    page.route('**/api/inventory/stream',lambda route:route.abort())
                    page.route('**/api/reconcile',lambda route:route.fulfill(status=202,json={'id':'owned'}))
                    page.route('**/api/reconciliation',lambda route:route.fulfill(json={
                        'comparison':{'id':'owned','state':'complete'},'inventory_accuracy':accuracy}))
                    page.goto(f'http://127.0.0.1:{server.server_port}/#discrepancies')
                    badge = page.locator('#discrepancy-count')
                    expect(badge).to_have_text('1')
                    expect(badge).to_have_class('has-errors')
                    expect(page.locator('#accuracy-findings')).to_contain_text('Web — Health check failed.')
                    page.get_by_role('button',name='Reconcile now').click()
                    expect(page.locator('#reconcile-status')).to_contain_text('Comparison finished · 2 of 2',timeout=8000)
                    page.locator('#services-tab').click()
                    expect(badge).to_have_text('1')
                    expect(page.locator('#cards')).to_contain_text('Web')
                    assert not errors, errors
                    page.close()
                    print(f'PASS {width}px: React badge, scoped receipt fallback and Services navigation',flush=True)
            finally:
                browser.close()
    finally:
        server.shutdown()
