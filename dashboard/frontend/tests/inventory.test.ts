import { test } from 'node:test';
import assert from 'node:assert/strict';
import { DashboardInventory } from '../inventory.ts';
import type { InventorySnapshot, ComparisonReport } from '../inventory.ts';

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(done => { resolve = done; });
  return { promise, resolve };
}
const accuracy = { checked: 30, hosts: [], findings: [], coverage: { verified: 6, expected: 6, unconfigured: 0 } };
const snapshot = (time: number): InventorySnapshot => ({
  machines: [], services: [], backups: [], fetched_at: time, server_time: time,
});

test('an unrelated completion and an old initial response cannot overwrite newer inventory', async () => {
  const accepted = deferred<{ id: string }>();
  const status = deferred<ComparisonReport>();
  const store = new DashboardInventory({
    startComparison: () => accepted.promise,
    readComparison: () => status.promise,
    wait: async () => {},
  });
  const comparing = store.reconcile();
  store.apply({ ...snapshot(30), comparison: { id: 'unrelated', state: 'complete' }, inventory_accuracy: accuracy });
  store.apply(snapshot(10));
  assert.equal(store.read().inventory.fetched_at, 30);
  assert.equal(store.read().comparison.phase, 'posting');
  accepted.resolve({ id: 'mine' });
  await Promise.resolve();
  assert.deepEqual(store.read().comparison, { phase: 'waiting', id: 'mine' });
  status.resolve({ comparison: { id: 'mine', state: 'complete' }, inventory_accuracy: accuracy, fetched_at: 31 });
  await comparing;
  assert.deepEqual(store.read().comparison, { phase: 'complete', accuracy });
});

test('stream loss remains visible until a valid live event arrives', () => {
  const store = new DashboardInventory();
  store.apply(snapshot(15), 'stream');
  store.streamUnavailable('Reconnecting');
  store.apply(snapshot(16));
  assert.equal(store.read().streamWarning, 'Reconnecting');
  store.apply(snapshot(17), 'stream');
  assert.equal(store.read().streamWarning, null);
});

test('a matching receipt arriving before the POST response settles without polling', async () => {
  const accepted = deferred<{ id: string }>();
  const store = new DashboardInventory({
    startComparison: () => accepted.promise,
    readComparison: async () => assert.fail('A completed receipt needs no status reads'),
    wait: async () => {},
  });
  const comparing = store.reconcile();
  store.apply({ ...snapshot(30), comparison: { id: 'mine', state: 'complete' }, inventory_accuracy: accuracy }, 'stream');
  accepted.resolve({ id: 'mine' });
  await comparing;
  assert.equal(store.read().comparison.phase, 'complete');
});

test('a status request from a completed comparison cannot fail its successor', async () => {
  const firstStatus = deferred<ComparisonReport | Error>();
  const secondReceipt = deferred<{ id: string }>();
  let accepted = 0;
  const store = new DashboardInventory({
    startComparison: () => ++accepted === 1 ? Promise.resolve({ id: 'first' }) : secondReceipt.promise,
    readComparison: async () => {
      const status = await firstStatus.promise;
      if (status instanceof Error) throw status;
      return status;
    },
    wait: async () => {},
  });
  const first = store.reconcile();
  await new Promise(setImmediate);
  store.apply({ ...snapshot(30), comparison: { id: 'first', state: 'complete' }, inventory_accuracy: accuracy });
  const second = store.reconcile();
  firstStatus.resolve(new Error('Old status request failed'));
  await first;
  assert.equal(store.read().comparison.phase, 'posting');
  store.apply({ ...snapshot(31), comparison: { id: 'second', state: 'complete' }, inventory_accuracy: accuracy });
  secondReceipt.resolve({ id: 'second' });
  await second;
  assert.equal(store.read().comparison.phase, 'complete');
});

test('an invalid event leaves the last usable snapshot intact', () => {
  const store = new DashboardInventory();
  store.apply(snapshot(20));
  assert.throws(() => store.apply({ ...snapshot(21), backups: { items: [] } }, 'stream'), /Invalid inventory/);
  assert.equal(store.read().inventory.fetched_at, 20);
});

test('a startup event cannot hide Leases when the initial inventory response arrives out of order', () => {
  const store = new DashboardInventory();
  store.apply(snapshot(30), 'stream');
  store.apply({ ...snapshot(29), lease_review_enabled: true });
  assert.equal(store.read().inventory.fetched_at, 30);
  assert.equal(store.read().inventory.lease_review_enabled, true);
  store.apply({ ...snapshot(31), lease_review_enabled: false });
  store.apply({ ...snapshot(28), lease_review_enabled: true });
  store.apply(snapshot(32), 'stream');
  assert.equal(store.read().inventory.lease_review_enabled, false);
});
