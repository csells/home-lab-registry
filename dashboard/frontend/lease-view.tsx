import { Fragment, useSyncExternalStore } from 'react';
import { createPortal } from 'react-dom';
import type { Lease, LeaseList } from './leases.ts';

const date = (value: number) => new Date(value * 1000).toLocaleString();

function LeaseCard({ lease }: { lease: Lease }) {
  const fields = [
    ['Owner', lease.owner],
    lease.released_at ? ['Released', date(lease.released_at)] : ['Renew by', date(lease.deadline)],
    lease.hold_mode && ['Review hold', lease.hold_mode + ' · ' + lease.hold_reason],
    lease.reviewer && ['Reviewer', lease.reviewer],
    lease.reviewer_hold && ['Reviewer hold', lease.reviewer_hold + ' · ' + lease.reviewer_reason],
    lease.export_error && ['Export failure', lease.export_error],
    ['Recovery points', String(lease.checkpoint_count)],
    lease.last_verified_at && ['Last verified', date(lease.last_verified_at)],
    lease.pinned ? ['Latest checkpoint', 'Pinned'] : lease.retained_through && ['Retained until', date(lease.retained_through)],
  ];
  return <article className="lease">
    <header><h3>{lease.task}</h3><strong>{lease.phase.replaceAll('-', ' ')}</strong></header>
    <dl>{fields.map(field => field && <Fragment key={field[0]}><dt>{field[0]}</dt><dd>{field[1]}</dd></Fragment>)}</dl>
  </article>;
}

export function LeaseView({ leases, badge }: { leases: LeaseList; badge: HTMLElement }) {
  const state = useSyncExternalStore(leases.subscribe, leases.read);
  const status = state.asOf ? 'Updated ' + date(state.asOf) : state.loading
    ? state.scope === 'current' ? 'Loading current leases…' : 'Loading lease history…'
    : '';
  return <>
    {createPortal(<span id="lease-count">{state.counts?.current ?? '—'}</span>, badge)}
    <div className="inventory-tools"><div className="inventory-title">
      <h2 id="lease-title">Environment leases</h2><p id="lease-as-of" role="status">{status}</p>
    </div></div>
    <div role="group" aria-label="Lease views" id="lease-views" className="filter-row">
      <button type="button" className="stat" data-scope="current" aria-pressed={state.scope === 'current'} onClick={() => void leases.select('current')}>
        Current <span className="count" id="current-lease-count">{state.counts?.current ?? '—'}</span>
      </button>
      <button type="button" className="stat" data-scope="released" aria-pressed={state.scope === 'released'} onClick={() => void leases.select('released')}>
        History <span className="count" id="released-lease-count">{state.counts?.released ?? '—'}</span>
      </button>
    </div>
    <p id="lease-error" role="alert" hidden={!state.error}>{state.error}</p>
    <div id="leases">
      {state.rows.map(lease => <LeaseCard key={lease.id} lease={lease} />)}
      {!state.rows.length && !state.loading && state.asOf > 0 &&
        <p>{state.scope === 'current' ? 'No current leases.' : 'No released leases.'}</p>}
    </div>
    <button type="button" className="button secondary" id="older" hidden={state.cursor === null} disabled={state.loading}
      onClick={() => void leases.loadMore()}>Show more leases</button>
  </>;
}
