type Scope = 'current' | 'released';
type Cursor = number | string | null;

export interface Lease {
  id: string;
  task: string;
  owner: string;
  phase: string;
  deadline: number;
  released_at?: number | null;
  hold_mode?: string | null;
  hold_reason?: string | null;
  reviewer?: string | null;
  reviewer_hold?: string | null;
  reviewer_reason?: string | null;
  export_error?: string | null;
  checkpoint_count: number;
  last_verified_at?: number | null;
  retained_through?: number | null;
  pinned?: boolean;
}

interface LeaseCounts {
  as_of: number;
  counts: { current: number; released: number };
}

export interface LeasePage extends LeaseCounts {
  leases: Lease[];
  next_before: Cursor;
}

interface LeaseState {
  scope: Scope;
  rows: Lease[];
  cursor: Cursor;
  counts: LeaseCounts['counts'] | null;
  asOf: number;
  countsAsOf: number;
  loading: boolean;
  error: string | null;
}

type FetchPage = (scope: Scope, before: Cursor, signal: AbortSignal) => Promise<LeasePage>;

const fetchPage: FetchPage = async (scope, before, signal) => {
  const query = new URLSearchParams({ scope });
  if (before !== null) query.set('before', String(before));
  const response = await fetch('/api/leases?' + query, {
    cache: 'no-store',
    signal: AbortSignal.any([signal, AbortSignal.timeout(12000)]),
  });
  if (!response.ok) throw new Error('HTTP ' + response.status);
  const page = await response.json();
  if (!Array.isArray(page?.leases) || !page.counts || typeof page.as_of !== 'number') {
    throw new Error('Invalid lease page');
  }
  return page;
};

export class LeaseList {
  private fetchPage: FetchPage;
  private request: AbortController | null = null;
  private listeners = new Set<() => void>();
  private state: LeaseState = {
    scope: 'current',
    rows: [],
    cursor: null,
    counts: null,
    asOf: 0,
    countsAsOf: 0,
    loading: false,
    error: null,
  };

  constructor(transport: FetchPage = fetchPage) {
    this.fetchPage = transport;
  }

  read = () => this.state;

  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => { this.listeners.delete(listener); };
  };

  private publish(change: Partial<LeaseState>) {
    this.state = { ...this.state, ...change };
    for (const listener of this.listeners) listener();
  }

  updateCounts(result: LeaseCounts) {
    if (result.as_of < this.state.countsAsOf) return;
    this.publish({ counts: result.counts, countsAsOf: result.as_of });
  }

  select(scope: Scope) {
    this.publish({ scope, rows: [], cursor: null, asOf: 0 });
    return this.load();
  }

  refresh() {
    return this.load();
  }

  loadMore() {
    if (this.state.loading || this.state.cursor === null) return;
    return this.load(this.state.cursor);
  }

  hide() {
    this.request?.abort();
    this.request = null;
  }

  private async load(cursor: Cursor = null) {
    this.request?.abort();
    const controller = new AbortController();
    this.request = controller;
    this.publish({ loading: true, error: null });
    try {
      const result = await this.fetchPage(this.state.scope, cursor, controller.signal);
      if (controller.signal.aborted) return;
      this.updateCounts(result);
      this.publish({
        rows: cursor === null ? result.leases : [...this.state.rows, ...result.leases],
        cursor: result.next_before,
        asOf: result.as_of,
      });
    } catch {
      if (!controller.signal.aborted) {
        this.publish({ asOf: 0, error: 'Leases could not be refreshed. Try again.' });
      }
    } finally {
      if (this.request === controller) {
        this.request = null;
        this.publish({ loading: false });
      }
    }
  }
}
