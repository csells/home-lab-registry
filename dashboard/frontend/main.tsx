import { useSyncExternalStore } from 'react';
import { createPortal } from 'react-dom';
import { createRoot } from 'react-dom/client';
import { DashboardInventory } from './inventory.ts';
import { age, describeDiscrepancies, findingGroups, tally } from './discrepancies.ts';
import type { Finding } from './inventory.ts';

declare global {
  interface Window {
    labDashboardInventory: DashboardInventory;
  }
}

const store = new DashboardInventory();
window.labDashboardInventory = store;

function FindingList({ items }: { items: Finding[] }) {
  return <ul>{items.map((item, index) => <li key={JSON.stringify([item.category, item.target, item.guest_id, item.code, index])}>
    {item.target}{item.guest_id ? ' / ' + item.guest_id : ''} — {item.message}{item.name ? ' (' + item.name + ')' : ''}
  </li>)}</ul>;
}

function Discrepancies({ badge }: { badge: HTMLElement }) {
  const state = useSyncExternalStore(store.subscribe, store.read);
  const now = Date.now() / 1000 + state.clockOffset;
  const report = describeDiscrepancies(state.inventory, now);
  const progress = state.comparison;
  const pending = progress.phase === 'posting' || progress.phase === 'waiting';
  let comparisonStatus = '';
  if (pending) comparisonStatus = 'Checking registered deployments against current host inventories…';
  if (progress.phase === 'failed') comparisonStatus = 'Could not complete reconciliation. ' + progress.message;
  if (progress.phase === 'complete') {
    const checks = progress.accuracy.hosts
      .filter(host => host.status === 'verified' && host.checked !== undefined)
      .map(host => host.checked!);
    const { verified, expected } = progress.accuracy.coverage;
    comparisonStatus = 'Comparison finished · ' + verified + ' of ' + expected + ' configured hosts verified';
    if (checks.length) comparisonStatus += ' · Oldest verified report: ' + age(Math.min(...checks), now);
  }

  return <>
    {createPortal(<span id="discrepancy-count" aria-live="polite" className={report.problems ? 'has-errors' : undefined}>
      {state.inventory.fetched_at ? report.problems : '—'}
    </span>, badge)}
    <div className="accuracy-heading">
      <div><h2 id="discrepancies-title">Discrepancies</h2><p id="accuracy-summary" role="status">{report.summary}</p></div>
      <button id="reconcile" type="button" className="button secondary" disabled={pending} onClick={() => void store.reconcile()}>
        Reconcile now
      </button>
    </div>
    <p id="reconcile-status" role="status">{comparisonStatus}</p>
    <div id="accuracy-details" hidden={report.findings.length === 0}>
      <div id="accuracy-findings">
        {findingGroups.map(([category, title]) => {
          const items = report.findings.filter(item => item.category === category);
          if (!items.length) return null;
          const heading = title + ' (' + tally(items) + ')';
          return category === 'coverage'
            ? <details key={category} className="accuracy-group"><summary>{heading}</summary><FindingList items={items} /></details>
            : <section key={category} className="accuracy-group"><h3>{heading}</h3><FindingList items={items} /></section>;
        })}
      </div>
    </div>
    <p id="accuracy-scope">{report.scope}</p>
  </>;
}

const section = document.getElementById('discrepancies-panel');
const badge = document.getElementById('discrepancy-badge-slot');
if (section && badge) createRoot(section).render(<Discrepancies badge={badge} />);
