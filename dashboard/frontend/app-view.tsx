import { useEffect, useRef, useState, useSyncExternalStore } from 'react';
import { MachineCard, ServiceCard } from './cards.tsx';
import { connectDashboard, refreshCatalog } from './dashboard-connection.ts';
import { Discrepancies } from './discrepancy-view.tsx';
import { describeDiscrepancies } from './discrepancies.ts';
import type { DashboardInventory } from './inventory.ts';
import { LeaseView } from './lease-view.tsx';
import type { LeaseList } from './leases.ts';
import { mountResourceGraph } from './resource-graph.ts';
import type { GraphRecord } from './resource-graph.ts';
import { health, resources, statuses } from './resources.ts';
import type { Health, ResourceFilters, ResourceView } from './resources.ts';

const views = ['machines', 'discrepancies', 'services', 'backups', 'map', 'leases'] as const;
type View = typeof views[number];

const labels: Record<View, string> = {
  machines: 'Machines', discrepancies: 'Discrepancies', services: 'Services', backups: 'Backups', map: 'Resources', leases: 'Leases',
};

const description: Record<ResourceView, string> = {
  machines: 'Physical hosts and virtual environments. Stopped states are labeled with their observation time.',
  services: 'Registered applications and infrastructure. Health reports do not imply every service is running.',
  backups: 'Scheduled jobs handled by the central backup service.',
  map: 'Physical machines, their VMs and containers, and the services they run.',
};

const placeholder: Record<ResourceView, string> = {
  machines: 'Find a machine…', services: 'Find a service…', backups: 'Find a backup job…', map: 'Find a machine or service…',
};

const blankFilters: ResourceFilters = { health: 'all', host: '', text: '' };

function urlView(): View {
  const value = location.hash.slice(1);
  return views.find(view => value === view) ?? 'machines';
}

function filterLabel(status: Health | 'all', machine: boolean) {
  if (machine && status === 'healthy') return 'Reachable';
  if (machine && status === 'unhealthy') return 'Check failed';
  return status[0].toUpperCase() + status.slice(1);
}

function MapCards({ items, matching }: { items: GraphRecord[]; matching: string[] }) {
  const ref = useRef<HTMLElement>(null);
  const graph = JSON.stringify([items, matching]);
  useEffect(() => {
    if (!ref.current) return;
    const [records, visible]: [GraphRecord[], string[]] = JSON.parse(graph);
    return mountResourceGraph(ref.current, records, new Set(visible));
  }, [graph]);
  return <section ref={ref} id="cards" aria-label="Resources" />;
}

export function App({ store, leases }: { store: DashboardInventory; leases: LeaseList }) {
  const state = useSyncExternalStore(store.subscribe, store.read);
  const leaseState = useSyncExternalStore(leases.subscribe, leases.read);
  const { inventory } = state;
  const now = Date.now() / 1000 + state.clockOffset;
  const [view, setView] = useState(urlView);
  const [filters, setFilters] = useState<ResourceFilters>(() => ({ ...blankFilters, text: new URLSearchParams(location.search).get('q') || '' }));
  const [pageVisible, setVisible] = useState(!document.hidden);
  const [refreshing, setRefreshing] = useState(false);
  const [toast, announce] = useState('');
  const nav = useRef<HTMLElement>(null);
  const leasesEnabled = inventory.lease_review_enabled === true;
  const showingLeases = view === 'leases' && leasesEnabled;
  const resourceView = view !== 'leases' && view !== 'discrepancies' ? view : undefined;
  const discrepancy = describeDiscrepancies(inventory, now);

  function select(next: View, host = '') {
    if (location.hash !== '#' + next) history.pushState(null, '', '#' + next);
    setFilters({ ...blankFilters, host });
    setView(next);
  }

  useEffect(() => connectDashboard(store, setVisible), [store]);

  useEffect(() => {
    function restore() {
      setView(urlView());
      setFilters(blankFilters);
    }
    window.addEventListener('popstate', restore);
    return () => window.removeEventListener('popstate', restore);
  }, []);

  useEffect(() => {
    if (view !== 'leases' || inventory.lease_review_enabled !== false) return;
    history.replaceState(null, '', '#machines');
    setView('machines');
  }, [view, inventory.lease_review_enabled]);

  useEffect(() => {
    if (!showingLeases) return;
    void leases.select('current');
    return () => leases.hide();
  }, [showingLeases, leases]);

  const leaseReport = inventory.lease_issues;
  useEffect(() => {
    if (leaseReport?.status === 'verified') leases.updateCounts(leaseReport);
  }, [leaseReport, leases]);

  useEffect(() => {
    const container = nav.current;
    const tab = container?.querySelector<HTMLButtonElement>('[aria-pressed="true"]');
    if (!container || !tab) return;
    const bounds = container.getBoundingClientRect();
    const target = tab.getBoundingClientRect();
    if (target.right > bounds.right) container.scrollLeft += target.right - bounds.right;
    if (target.left < bounds.left) container.scrollLeft -= bounds.left - target.left;
  }, [view, leasesEnabled]);

  async function refresh() {
    if (showingLeases) {
      await leases.refresh();
      return;
    }
    setRefreshing(true);
    try {
      await refreshCatalog(store);
    } finally {
      setRefreshing(false);
    }
  }

  const message = state.streamWarning || state.requestWarning || inventory.error || '';
  const checked = inventory.catalog?.snapshot_at || inventory.catalog?.checked || inventory.fetched_at;
  const catalog = checked
    ? (inventory.catalog?.snapshot_at ? 'Preview catalog copied ' : 'Catalog last read ') +
      new Date(checked * 1000).toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' })
    : 'Connecting to the lab…';
  const missingFeed = inventory.catalog && inventory.catalog.feed !== 'connected' ? 'Catalog feed unavailable · ' : '';
  const badges = {
    machines: <span id="machine-count">{inventory.machines.length}</span>,
    services: <span id="service-count">{inventory.services.length}</span>,
    backups: <span id="backup-count">{inventory.backups.length}</span>,
    discrepancies: <span id="discrepancy-count" aria-live="polite" className={discrepancy.problems ? 'has-errors' : undefined}>
      {inventory.fetched_at ? discrepancy.problems : '—'}
    </span>,
    leases: <span id="lease-count">{leaseState.counts?.current ?? '—'}</span>,
    map: null,
  };

  const result = resourceView ? resources(inventory, resourceView, filters, now) : undefined;
  const map = resourceView === 'map';
  const machine = resourceView === 'machines';
  const noun = map ? 'resources' : resourceView;

  return <>
    <a href="#content" className="skip-link">Skip to inventory</a>
    <header className="masthead"><a className="brand" href="#machines" aria-label="Home lab machines" onClick={event => {
      event.preventDefault();
      select('machines');
    }}>
      <span className="brand-mark" aria-hidden="true">⌘</span><span>Home lab<span className="brand-sub">CONTROL ROOM</span></span>
    </a></header>
    <main id="content">
      <section className="page-heading">
        <div><p className="eyebrow">YOUR CONTROL ROOM</p><h1>Make yourself at home.</h1>
          <p className="intro">Open a machine. Check a service. Know what needs attention.</p></div>
        <div className="refresh-block"><button id="refresh" type="button" className="button secondary" disabled={refreshing}
          onClick={() => void refresh()}>↻ Refresh</button><span id="updated" hidden={view === 'leases'}>{missingFeed + catalog}</span></div>
      </section>
      <nav ref={nav} aria-label="Inventory views">
        {views.map(tab => <button key={tab} id={tab + '-tab'} type="button" aria-pressed={view === tab && (tab !== 'leases' || leasesEnabled)}
          aria-controls={tab === 'discrepancies' ? 'discrepancies-panel' : tab === 'leases' ? 'lease-panel' : 'inventory-panel'}
          hidden={tab === 'leases' && !leasesEnabled} onClick={() => select(tab)}>
          {labels[tab]} {badges[tab]}
        </button>)}
      </nav>
      <div id="error" role="alert" hidden={!message}>{message}</div>
      <Discrepancies store={store} hidden={view !== 'discrepancies'} />
      <div id="inventory-panel" hidden={!resourceView}>
        {resourceView && result && <>
          <section className="inventory-tools" aria-label="Inventory controls">
            <div className="inventory-title"><h2 id="title">{labels[resourceView]}</h2><p id="view-description">{description[resourceView]}</p></div>
            <div className="search-tools">
              <label className="search-field"><span className="sr-only">Search</span><input id="search" type="search"
                placeholder={placeholder[resourceView]} autoComplete="off" value={filters.text}
                onChange={event => setFilters({ ...filters, text: event.currentTarget.value })} /></label>
              <label><span className="sr-only">Filter by host</span><select id="host" value={result.host}
                onChange={event => setFilters({ ...filters, host: event.currentTarget.value })}>
                <option value="">All hosts</option>
                {result.hosts.map(host => <option key={host} value={host}>{host}</option>)}
              </select></label>
            </div>
          </section>
          <section id="health-filters" className="filter-row" aria-label="Health filters">
            {(['all', ...statuses] as const).map(status => <button type="button" key={status} className="stat" data-filter={status}
              aria-pressed={status === filters.health} onClick={() => setFilters({ ...filters, health: status })}>
              {filterLabel(status, machine)}<span className="count">{result.counts[status]}</span>
            </button>)}
          </section>
          <p id="summary" className="inventory-summary" aria-live="polite">{result.rows.length} of {result.items.length} {noun}</p>
          {map && result.rows.length ? <MapCards items={result.items.map(record => ({
            id: record.id, name: record.display_name || record.name, machine_id: record.machine_id, dependencies: record.dependencies,
            kind: record.kind === 'machine' ? record.machine_type : 'service', status: health(record, now),
          }))} matching={result.rows.map(record => record.id)} />
          : <section id="cards" className={map ? '' : machine ? 'machines' : 'services'} aria-label={labels[resourceView]}>
            {!result.rows.length && <p className="empty-results">No {noun} match these filters.</p>}
            {!map && result.rows.map(record => machine ? <MachineCard key={record.id} record={record} inventory={inventory} now={now}
              pageVisible={pageVisible} announce={announce} showServices={() => select('services', record.host)} />
              : <ServiceCard key={record.id} record={record} now={now} announce={announce} showHost={() => select('machines', record.host)} />)}
          </section>}
        </>}
      </div>
      <LeaseView leases={leases} hidden={!showingLeases} />
      <footer><strong>Home lab</strong><span>Live charts while viewing · Guest inventory changes stream automatically.</span></footer>
      <div id="toast" role="status" className="sr-only">{toast}</div>
    </main>
  </>;
}
