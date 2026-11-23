import { test } from 'node:test';
import assert from 'node:assert/strict';
import { refreshCatalog } from '../dashboard-connection.ts';
import { DashboardInventory } from '../inventory.ts';

test('Refresh reads the current inventory when the origin has read-only access', async t => {
  const fetch = t.mock.method(globalThis, 'fetch', async (url: string, init?: RequestInit) => {
    if (url === 'api/refresh' && init?.method === 'POST') return new Response(null, { status: 403 });
    assert.equal(url, 'api/inventory');
    assert.equal(init?.cache, 'no-store');
    return Response.json({ machines: [], services: [], backups: [], fetched_at: 42, server_time: 42 });
  });
  const store = new DashboardInventory();
  await refreshCatalog(store);
  assert.equal(fetch.mock.callCount(), 2);
  assert.equal(store.read().inventory.fetched_at, 42);
});

test('Refresh reports server and login errors without falling back to an inventory read', async t => {
  for (const status of [401, 503]) {
    const fetch = t.mock.method(globalThis, 'fetch', async () => new Response(null, { status }));
    const store = new DashboardInventory();
    await refreshCatalog(store);
    assert.equal(fetch.mock.callCount(), 1);
    assert.match(JSON.stringify(store.read()), new RegExp(`Catalog refresh failed: HTTP ${status}`));
    fetch.mock.restore();
  }
});
