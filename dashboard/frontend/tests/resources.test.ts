import assert from 'node:assert/strict';
import { test } from 'node:test';
import { resources } from '../resources.ts';
import type { InventoryRecord, InventorySnapshot } from '../inventory.ts';

function row(id: string, fields: Partial<InventoryRecord> = {}): InventoryRecord {
  return { id, name: id, host: 'worker', lifecycle: 'active', capabilities: [], ...fields };
}

test('inventory filters use observed power and keep backups out of the resource graph', () => {
  const lab: InventorySnapshot = {
    machines: [
      row('VM', { kind: 'machine', machine_type: 'virtual-machine',
        runtime: { state: 'stopped', expires: 50 }, health: { status: 'unhealthy', expires: 50 } }),
      row('Host', { kind: 'machine', machine_type: 'machine', host: 'other' }),
    ],
    services: [row('Application', { kind: 'service', capabilities: ['web'] })],
    backups: [row('Backup', { kind: 'backup', health: { status: 'unhealthy', expires: 50 } })],
  };
  const space = resources(lab, 'map', { health: 'all', host: 'worker', text: 'web' }, 10);
  assert.deepEqual(space.rows.map(row => row.id), ['Application']);
  assert.equal(space.counts.all, 3);
  assert.equal(space.counts.stopped, 1);
  assert.equal(space.counts.unhealthy, 0);
  assert.deepEqual(space.hosts, ['other', 'worker']);
  assert.deepEqual(resources(lab, 'backups', { health: 'unhealthy', host: '', text: '' }, 10).rows.map(row => row.id), ['Backup']);
});
