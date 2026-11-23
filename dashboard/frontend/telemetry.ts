import type { MetricSample } from './inventory.ts';

export const metricNames = ['CPU','RAM','GPU','DISK'] as const;
export type MetricName = typeof metricNames[number];

function finite(value?: number | null): number | null {
  return typeof value === 'number' && Number.isFinite(value) ? value : null;
}

export function metricValue(sample: MetricSample, name: MetricName): number | null {
  switch (name) {
    case 'CPU':
      return finite(sample.cpu_percent);
    case 'RAM':
      return sample.memory_total && sample.memory_used !== undefined
        ? finite(100 * sample.memory_used / sample.memory_total) : null;
    case 'GPU':
      return sample.gpu?.status === 'available' ? finite(sample.gpu.percent) : null;
    case 'DISK': {
      const storage = (sample.disks ?? []).filter(disk =>
        disk.total > 0 && (!disk.checked || !sample.checked || sample.checked - disk.checked <= 120));
      return storage.length ? finite(Math.max(...storage.map(disk => 100 * disk.used / disk.total))) : null;
    }
  }
}

interface Bucket {
  value: number;
  sample: MetricSample;
}

export function chart(history: MetricSample[], name: MetricName, now: number) {
  const start = now - 600;
  const buckets = new Map<number, Bucket>();
  for (const sample of history) {
    if (sample.status !== 'available' || sample.checked == null || sample.checked < start || sample.checked > now) continue;
    const value = metricValue(sample, name);
    if (value === null) continue;
    const key = Math.min(29, Math.floor((sample.checked - start) / 20));
    if ((buckets.get(key)?.value ?? -Infinity) < value) buckets.set(key, { value, sample });
  }

  const values = [...buckets.values()].map(entry => entry.value);
  let low = 0;
  let high = 100;
  if (values.length > 0 && (name === 'CPU' || name === 'GPU')) {
    high = Math.min(100, Math.max(10, Math.ceil(Math.max(...values) / 10) * 10));
  }
  if (values.length > 0 && name === 'RAM') {
    low = Math.max(0, Math.floor(Math.min(...values) / 5) * 5);
    high = Math.min(100, Math.max(low + 10, Math.ceil(Math.max(...values) / 5) * 5));
    low = Math.min(low, high - 10);
  }

  const bars = Array.from({ length: 30 }, (_, index) => {
    const entry = buckets.get(index);
    const height = entry ? Math.max(1, Math.min(1, Math.max(0, (entry.value - low) / (high - low))) * 50) : 1;
    return { ...entry, height };
  });
  return { low, high, scale: low + '–' + high + '%', bars };
}
