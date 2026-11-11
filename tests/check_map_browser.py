"""Exercise ELK resource layout through the actual dashboard static routes and CSP."""
import os
import pathlib
import sys
import threading
from playwright.sync_api import sync_playwright, expect

ROOT=pathlib.Path(__file__).resolve().parents[1]
sys.path[:0]=[str(ROOT/'dashboard'),str(ROOT/'registry')]
import server


def row(identity,kind='machine',parent=None,host='synthetic-host',dependencies=None):
    return dict(id=identity,name=identity,display_name=identity,host=host,
                kind='service' if kind=='service' else 'machine',machine_type=kind,
                machine_id=parent,dependencies=dependencies or [],capabilities=[],endpoints={},
                health={'status':'unknown'},metrics={'status':'unconfigured'})


catalog=dict(machines=[row('Linux'),row('Mac',parent='Linux',host='synthetic-mac'),
                       row('Container','container','Linux'),row('VM','virtual-machine','Linux'),
                       row('Orphan','container','missing'),row('Cycle-a','container','Cycle-b'),
                       row('Cycle-b','container','Cycle-a')],
             services=[row('Native app','service','Linux',dependencies=['Guest app']),
                       row('Guest app','service','Container',dependencies=['Missing backend']),
                       row('<img src=x onerror=alert(1)>','service','VM')],backups=[row('Backup job','backup-job')])
catalog['machines'][3]['runtime']={'state':'stopped','expires':4102444800}
catalog['machines'][0]['metrics']={'status':'waiting'}
http=server.ThreadingHTTPServer(('127.0.0.1',0),server.Handler);http.auth=''
threading.Thread(target=http.serve_forever,daemon=True).start()
try:
    with sync_playwright() as p:
        launch={}
        if os.environ.get('REVIEW_CHROMIUM'):launch['executable_path']=os.environ['REVIEW_CHROMIUM']
        browser=p.chromium.launch(**launch)
        try:
            for width in (1440,393):
                page=browser.new_page(viewport={'width':width,'height':1000});errors=[]
                page.on('pageerror',lambda e:errors.append(str(e)))
                page.route('**/api/inventory',lambda route:route.fulfill(json=catalog))
                page.route('**/api/network-audit',lambda route:route.fulfill(json={'message':'Synthetic audit'}))
                page.route('**/api/metrics/stream?*',lambda route:route.fulfill(content_type='text/event-stream',body='event: metrics\ndata: {"status":"waiting"}\n\n'))
                page.goto(f'http://127.0.0.1:{http.server_port}/#map')
                page.locator('#cards[data-layout-ready="true"]').wait_for()
                expect(page.locator('.resource-node[data-resource-id]')).to_have_count(10)
                expect(page.locator('.resource-edge')).to_have_count(1)
                assert page.locator('#cards button').count()==0,'graph is read-only'
                box=page.locator('.graph-viewport svg').bounding_box();assert box['width']>200 and box['height']>200,box
                assert 'outside this view' in page.locator('.graph-status').inner_text()
                assert page.locator('#cards img').count()==0
                assert 'Backup job' not in page.locator('#cards').inner_text()
                page.locator('#search').fill('Guest app')
                expect(page.locator('.resource-node[data-resource-id]')).to_have_count(3)
                expect(page.locator('.resource-edge')).to_have_count(0)
                page.locator('#search').fill('')
                page.locator('#host').select_option('synthetic-mac')
                expect(page.locator('.resource-node[data-resource-id]')).to_have_count(1)
                page.locator('#host').select_option('')
                page.locator('[data-filter="stopped"]').click()
                expect(page.locator('.resource-node[data-resource-id="VM"]')).to_have_count(1)
                expect(page.locator('.resource-node[data-resource-id="Container"]')).to_have_count(0)
                page.locator('[data-filter="all"]').click()
                expect(page.locator('.resource-node[data-resource-id]')).to_have_count(10)
                assert page.evaluate('document.documentElement.scrollWidth<=innerWidth')
                assert not errors,errors
                page.locator('#machines-tab').click();expect(page.locator('#cards .machine-card')).to_have_count(7)
                page.locator('#services-tab').click();expect(page.locator('#cards .service-card')).to_have_count(3)
                page.locator('#backups-tab').click();expect(page.locator('#cards .service-card')).to_have_count(1)
                page.close()
                print(f'PASS {width}px: real static routes/CSP, ELK worker, resources, routes, actual-size read-only rendering, filters and original tabs',flush=True)
        finally:browser.close()
finally:http.shutdown();http.server_close()
