export interface Finding {
  category: string;
  target: string;
  message: string;
  code?: string;
  machine_id?: string;
  guest_id?: string;
  name?: string;
  count?: number;
}

interface Observation {
  status?: string;
  expires?: number;
  checked?: number;
  evidence?: string;
}

export interface AccessRoute {
  id: string;
  label: string;
  url?: string;
  username?: string | null;
  username_required?: boolean;
  password_available?: boolean;
  password_note?: string;
  browser_terminal?: string | null;
  note?: string;
}

export interface MetricSample {
  status: string;
  checked?: number;
  expires?: number;
  cpu_percent?: number;
  memory_total?: number;
  memory_used?: number;
  gpu?: { status: string; percent?: number | null; note?: string };
  disks?: { name: string; used: number; total: number; checked?: number }[];
  warnings?: string[];
}

export interface Metrics extends MetricSample {
  history?: MetricSample[];
}

export interface InventoryRecord {
  id: string;
  name: string;
  host: string;
  display_name?: string;
  lifecycle: 'active' | 'planned' | 'retired';
  capabilities: string[];
  dependencies?: string[];
  kind?: 'machine' | 'service' | 'backup';
  machine_type?: 'machine' | 'virtual-machine' | 'container';
  machine_id?: string | null;
  model?: string | null;
  os?: string | null;
  deployment?: string;
  endpoints?: Record<string, string>;
  access?: AccessRoute[];
  console_login?: { username?: string; credential_id: string; password_available?: boolean } | null;
  config_backup?: { status?: string; locations?: { host: string; path: string }[]; notes?: string };
  metrics?: Metrics;
  health?: Observation;
  runtime?: { state?: string; expires?: number; checked?: number };
}

export interface HostReport {
  machine_id: string;
  status: string;
  expires?: number;
  checked?: number;
  hardware?: { expires: number; checked: number; architecture: string; os: string; logical_cpus: number; memory_bytes: number };
}

export interface InventoryAccuracy {
  checked: number;
  hosts: HostReport[];
  findings: Finding[];
  deployments?: { record_id: string; state: string; expires: number }[];
  coverage: { verified: number; expected: number; unconfigured: number };
}

interface ComparisonReceipt {
  id: string;
  state: 'pending' | 'complete' | 'failed';
}

export interface ComparisonReport {
  comparison?: ComparisonReceipt | null;
  inventory_accuracy?: InventoryAccuracy | null;
  fetched_at?: number | null;
}

export interface InventorySnapshot extends ComparisonReport {
  machines: InventoryRecord[];
  services: InventoryRecord[];
  backups: InventoryRecord[];
  server_time?: number;
  error?: string | null;
  lease_review_enabled?: boolean;
  catalog?: { checked?: number | null; snapshot_at?: number | null; feed: string };
  address_audit?: { status: string; at: string; warnings: number; unreserved_static?: number | null } | null;
  lease_issues?: {
    status: 'checking' | 'unavailable';
  } | {
    status: 'verified';
    issues: { code: 'export-failed' | 'overdue'; task: string }[];
    counts: { current: number; released: number };
    as_of: number;
  } | null;
}

type Comparison = { phase: 'idle' | 'posting' } | { phase: 'waiting'; id: string }
  | { phase: 'complete'; accuracy: InventoryAccuracy } | { phase: 'failed'; message: string };

interface InventoryState {
  inventory: InventorySnapshot;
  comparison: Comparison;
  streamWarning: string | null;
  requestWarning: string | null;
  clockOffset: number;
  updateCount: number;
}

interface ComparisonTransport {
  startComparison(): Promise<{ id: string }>;
  readComparison(): Promise<ComparisonReport>;
  wait(): Promise<void>;
}

async function fetchJson<T>(path: string, options: RequestInit = {}): Promise<T> {
  const response = await fetch(path, { cache: 'no-store', signal: AbortSignal.timeout(8000), ...options });
  if (!response.ok) throw new Error('HTTP ' + response.status);
  return response.json();
}

const browserTransport: ComparisonTransport = {
  startComparison: () => fetchJson('api/reconcile', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', 'X-Lab-Action': 'reconcile' },
    body: '{}',
    signal: AbortSignal.timeout(12000),
  }),
  readComparison: () => fetchJson('api/reconciliation'),
  wait: () => new Promise(resolve => setTimeout(resolve, 4000)),
};

function inventoriesOrdered(before: InventorySnapshot, after: InventorySnapshot): boolean {
  const lastChecked = before.fetched_at ?? 0;
  const checked = after.fetched_at ?? 0;
  if (lastChecked !== checked) return lastChecked < checked;
  return (before.server_time ?? 0) <= (after.server_time ?? 0);
}

export class DashboardInventory {
  private state: InventoryState = {
    inventory: { machines: [], services: [], backups: [] },
    comparison: { phase: 'idle' },
    streamWarning: null,
    requestWarning: null,
    clockOffset: 0,
    updateCount: 0,
  };
  private listeners = new Set<() => void>();
  private transport: ComparisonTransport;
  private comparisonRun = 0;

  constructor(transport: ComparisonTransport = browserTransport) {
    this.transport = transport;
  }

  read = () => this.state;

  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => { this.listeners.delete(listener); };
  };

  private publish(change: Partial<InventoryState>) {
    this.state = { ...this.state, ...change };
    for (const listener of this.listeners) listener();
  }

  tick() {
    this.publish({ updateCount: this.state.updateCount + 1 });
  }

  streamUnavailable(message: string) {
    if (this.state.streamWarning !== message) this.publish({ streamWarning: message });
  }

  requestFailed(message: string) {
    this.publish({ requestWarning: message });
  }

  apply(payload: unknown, source: 'http' | 'stream' = 'http'): boolean {
    let snapshot = parseInventory(payload);
    const current = this.state.inventory;
    if ((current.fetched_at || current.server_time) && !inventoriesOrdered(current, snapshot)) {
      this.observeComparison(snapshot);
      if (source === 'stream' && this.state.streamWarning) this.publish({ streamWarning: null });
      return false;
    }
    // Direct inventory reads include the feature toggle. Older event schemas omit it.
    if (snapshot.lease_review_enabled === undefined && current.lease_review_enabled !== undefined) {
      snapshot = { ...snapshot, lease_review_enabled: current.lease_review_enabled };
    }
    this.publish({
      inventory: snapshot,
      clockOffset: snapshot.server_time ? snapshot.server_time - Date.now() / 1000 : this.state.clockOffset,
      streamWarning: source === 'stream' ? null : this.state.streamWarning,
      requestWarning: source === 'http' ? null : this.state.requestWarning,
    });
    this.observeComparison(snapshot);
    return true;
  }

  private observeComparison(report: ComparisonReport) {
    const operation = this.state.comparison;
    const receipt = report.comparison;
    if (operation.phase !== 'waiting' || receipt?.id !== operation.id || receipt.state === 'pending') return;
    const accuracy = report.inventory_accuracy;
    if (!accuracy || receipt.state === 'failed') {
      this.publish({ comparison: { phase: 'failed', message: 'Check registry connectivity and retry.' } });
      return;
    }
    const snapshot = this.state.inventory;
    const display = !snapshot.inventory_accuracy || accuracy.checked >= snapshot.inventory_accuracy.checked
      ? { ...snapshot, inventory_accuracy: accuracy }
      : snapshot;
    this.publish({ inventory: display, comparison: { phase: 'complete', accuracy } });
  }

  async reconcile() {
    if (this.state.comparison.phase === 'posting' || this.state.comparison.phase === 'waiting') return;
    const run = ++this.comparisonRun;
    const waiting = () => this.comparisonRun === run && this.read().comparison.phase === 'waiting';
    this.publish({ comparison: { phase: 'posting' } });
    try {
      const { id } = await this.transport.startComparison();
      if (typeof id !== 'string' || !id) throw new Error('Server returned no comparison ID');
      this.publish({ comparison: { phase: 'waiting', id } });
      this.observeComparison(this.state.inventory);
      const deadline = Date.now() + 90000;
      while (waiting() && Date.now() < deadline) {
        await this.transport.wait();
        if (!waiting()) break;
        const report = await this.transport.readComparison();
        if (waiting()) this.observeComparison(report);
      }
      if (waiting()) throw new Error('Timed out waiting for a completed reconciliation');
    } catch (error) {
      if (this.comparisonRun !== run || this.read().comparison.phase === 'complete') return;
      const message = error instanceof Error ? error.message : 'Unexpected reconciliation error';
      this.publish({ comparison: { phase: 'failed', message } });
    }
  }
}

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function recordsArray(value: unknown): boolean {
  return Array.isArray(value) && value.every(isObject);
}

export function parseInventory(value: unknown): InventorySnapshot {
  if (!isObject(value) || !recordsArray(value.machines) || !recordsArray(value.services) ||
      (value.backups !== undefined && !recordsArray(value.backups))) {
    throw new Error('Invalid inventory snapshot');
  }
  const accuracy = value.inventory_accuracy;
  if (accuracy != null && (!isObject(accuracy) || !recordsArray(accuracy.hosts) ||
      !recordsArray(accuracy.findings) || !isObject(accuracy.coverage) ||
      (accuracy.deployments !== undefined && !recordsArray(accuracy.deployments)))) {
    throw new Error('Invalid inventory accuracy report');
  }
  const leaseReport = value.lease_issues;
  if (leaseReport != null && (!isObject(leaseReport) || (leaseReport.status === 'verified' &&
      (!recordsArray(leaseReport.issues) || !isObject(leaseReport.counts))))) {
    throw new Error('Invalid inventory lease report');
  }
  const snapshot = value as unknown as InventorySnapshot;
  return { ...snapshot, backups: snapshot.backups ?? [] };
}
