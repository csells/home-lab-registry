import type { InventoryRecord, InventorySnapshot } from './inventory.ts';

export const statuses = ['healthy','unhealthy','stale','unknown','stopped','running'] as const;
export type Health = typeof statuses[number];
export type ResourceView = 'machines' | 'services' | 'backups' | 'map';

export interface ResourceFilters {
  health: 'all' | Health;
  host: string;
  text: string;
}

const rank: Record<Health, number> = { unhealthy: 0, stale: 1, unknown: 2, healthy: 3, running: 3, stopped: 4 };

function knownHealth(value?: string): value is Health {
  return !!value && Object.hasOwn(rank, value);
}

export function health(record: InventoryRecord, now: number): Health {
  const runtime = record.runtime;
  if (runtime?.expires) {
    if (runtime.expires < now) return 'unknown';
    if (runtime.state === 'stopped' || runtime.state === 'running' || runtime.state === 'unknown') return runtime.state;
  }
  const check = record.health;
  if (check?.expires && check.expires < now) return 'stale';
  return knownHealth(check?.status) ? check.status : 'unknown';
}

export function resources(inventory: InventorySnapshot, view: ResourceView, filters: ResourceFilters, now: number) {
  const items = view === 'map' ? [...inventory.machines, ...inventory.services] : inventory[view];
  const hosts = [...new Set(items.map(item => item.host))].sort();
  const host = hosts.includes(filters.host) ? filters.host : '';
  const text = filters.text.toLowerCase();
  const counts = { all: items.length, healthy: 0, unhealthy: 0, stale: 0, unknown: 0, stopped: 0, running: 0 };
  const rows = [];
  for (const item of items) {
    const status = health(item, now);
    counts[status]++;
    if (filters.health !== 'all' && status !== filters.health) continue;
    if (host && host !== item.host) continue;
    if (![item.name, item.host, item.id, ...(item.capabilities ?? [])].join(' ').toLowerCase().includes(text)) continue;
    rows.push(item);
  }
  rows.sort((a, b) => rank[health(a, now)] - rank[health(b, now)] || a.name.localeCompare(b.name));
  return { items, rows, hosts, counts, host };
}
