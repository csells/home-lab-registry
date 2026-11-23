"""Exercise the built Discrepancies UI with controlled responses and no browser SSE."""
import functools
import os
import pathlib
import threading
import time
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlsplit
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
                                'lease_review_enabled':True,
                                'services':[{'id':'web','name':'Web','host':'test','lifecycle':'active',
                                             'capabilities':[],'health':{'status':'unhealthy','expires':stamp+60}}],
                                'inventory_accuracy':accuracy}
                    page.route('**/api/inventory',lambda route:route.fulfill(json=snapshot))
                    page.route('**/api/inventory/stream',lambda route:route.abort())
                    page.route('**/api/reconcile',lambda route:route.fulfill(status=202,json={'id':'owned'}))
                    page.route('**/api/reconciliation',lambda route:route.fulfill(json={
                        'comparison':{'id':'owned','state':'complete'},'inventory_accuracy':accuracy}))
                    def leases(route):
                        query = parse_qs(urlsplit(route.request.url).query)
                        scope = query.get('scope', ['current'])[0]
                        current = scope == 'current'
                        more = 'before' in query
                        tasks = ['Current task'] if current else ['Older task' if more else 'Recent release']
                        route.fulfill(json={'as_of':stamp,'counts':{'current':1,'released':2},
                            'leases':[{'id':task,'task':task,'owner':'fixture','phase':'ready' if current else 'released',
                                       'deadline':stamp+60,'checkpoint_count':0} for task in tasks],
                            'next_before':None if current or more else 2})
                    page.route('**/api/leases?*', leases)
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
                    page.locator('#leases-tab').click()
                    expect(page.locator('#leases')).to_contain_text('Current task')
                    page.get_by_role('button',name='History 2').click()
                    expect(page.locator('#leases')).to_contain_text('Recent release')
                    page.get_by_role('button',name='Show more leases').click()
                    expect(page.locator('#leases')).to_contain_text('Older task')
                    expect(page.locator('#leases')).to_contain_text('Recent release')
                    expect(page.locator('#leases')).not_to_contain_text('Current task')
                    assert not errors, errors
                    page.close()
                    print(f'PASS {width}px: React badge, scoped receipt fallback, Services and paginated Leases',flush=True)
            finally:
                browser.close()
    finally:
        server.shutdown()
