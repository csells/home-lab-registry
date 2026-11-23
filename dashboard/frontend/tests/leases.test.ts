import { test } from 'node:test';
import assert from 'node:assert/strict';
import { LeaseList } from '../leases.ts';
import type { LeasePage } from '../leases.ts';

const page = (tasks: string[], asOf: number, cursor: number | null = null): LeasePage => ({
  counts: { current: 1, released: 2 },
  as_of: asOf,
  leases: tasks.map(task => ({
    id: task, task, owner: 'test', phase: 'ready', deadline: 200,
    checkpoint_count: 0,
  })),
  next_before: cursor,
});

test('opening Leases shows current work and history paginates separately', async () => {
  const calls: string[] = [];
  const leases = new LeaseList(async (scope, cursor) => {
    calls.push(scope + ':' + cursor);
    if (scope === 'current') return page(['active task'], 10);
    return cursor ? page(['old task'], 30) : page(['newer task'], 20, 2);
  });
  await leases.select('current');
  assert.deepEqual(leases.read().rows.map(row => row.task), ['active task']);
  await leases.select('released');
  await leases.loadMore();
  assert.deepEqual(leases.read().rows.map(row => row.task), ['newer task', 'old task']);
  assert.equal(leases.read().cursor, null);
  assert.deepEqual(calls, ['current:null', 'released:null', 'released:2']);
});

test('a delayed current response cannot replace History or its newer counts', async () => {
  let finishCurrent!: (value: LeasePage) => void;
  const current = new Promise<LeasePage>(resolve => { finishCurrent = resolve; });
  const leases = new LeaseList(async scope => scope === 'current' ? current : page(['history'], 20));
  const earlier = leases.select('current');
  await leases.select('released');
  finishCurrent({ ...page(['old response'], 10), counts: { current: 9, released: 11 } });
  await earlier;
  leases.updateCounts({ as_of: 15, counts: { current: 3, released: 5 } });
  assert.deepEqual(leases.read().rows.map(row => row.task), ['history']);
  assert.deepEqual(leases.read().counts, { current: 1, released: 2 });
});
