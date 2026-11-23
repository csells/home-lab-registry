import assert from 'node:assert/strict';
import test from 'node:test';
import { describeDiscrepancies } from '../discrepancies.ts';

test('published IP audits retain their status and observation time even with zero violations', () => {
  const at = '2026-11-23T08:17:21Z';
  const now = Date.parse(at) / 1000;
  for (const status of ['passed', 'incomplete', 'unknown']) {
    const report = describeDiscrepancies({
      machines: [], services: [], backups: [], fetched_at: now,
      address_audit: { at, status, warnings: 0, unreserved_static: 0 },
    }, now);
    assert.equal(report.problems, 0);
    assert.ok(report.scope.includes(status), report.scope);
    assert.ok(report.scope.includes(new Date(at).toLocaleString()), report.scope);
    assert.equal(report.findings.some(item => item.category === 'coverage' && item.target === 'IP reservation audit'),
      status !== 'passed');
  }
});
