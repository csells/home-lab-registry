import assert from 'node:assert/strict';
import { test } from 'node:test';
import { chart, metricValue } from '../telemetry.ts';

test('GPU history keeps real zero and peaks, and omits expired and unavailable samples', () => {
  const report = chart([
    { status: 'available', checked: 399, gpu: { status: 'available', percent: 90 } },
    { status: 'available', checked: 411, gpu: { status: 'available', percent: 0 } },
    { status: 'available', checked: 426, gpu: { status: 'available', percent: 4 } },
    { status: 'available', checked: 431, gpu: { status: 'available', percent: 27 } },
    { status: 'unavailable', checked: 461, gpu: { status: 'available', percent: 90 } },
  ], 'GPU', 1000);
  assert.equal(report.scale, '0–30%');
  assert.deepEqual(report.bars.filter(bar => bar.sample).map(bar => bar.value), [0, 27]);
  assert.equal(report.bars.length, 30);
  assert.equal(metricValue({ status: 'available', gpu: { status: 'not_applicable' } }, 'GPU'), null);
});
