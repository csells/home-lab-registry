import { useRef } from 'react';
import { age } from './discrepancies.ts';
import { CopyPassword, Details, ExternalLink, MachineAccess, Username } from './card-access.tsx';
import { serviceEndpoints } from './access.ts';
import { health } from './resources.ts';
import type { Health } from './resources.ts';
import type { InventoryRecord, InventorySnapshot } from './inventory.ts';
import { Telemetry } from './telemetry-view.tsx';

const machineHealth: Record<Health, string> = {
  healthy: 'Reachable', unhealthy: 'Check failed', unknown: 'Unknown', running: 'Running', stale: 'Stale', stopped: 'Stopped',
};

type Announce = (message: string) => void;

function ReadingTime({ stamp, now, absolute = false }: { stamp?: number; now: number; absolute?: boolean }) {
  if (!stamp) return <time>No reading</time>;
  const date = new Date(stamp * 1000);
  return <time dateTime={date.toISOString()} title={date.toLocaleString()}>
    {absolute ? date.toLocaleString() : age(stamp, now)}
  </time>;
}

function CardHeader({ record, machine, now }: { record: InventoryRecord; machine: boolean; now: number }) {
  const status = health(record, now);
  const checked = status === 'running' || status === 'stopped' ? record.runtime?.checked : record.health?.checked;
  return <header className="card-head">
    {machine && <span className="machine-symbol" aria-hidden="true">
      <svg viewBox="0 0 24 24"><rect x="4" y="3" width="16" height="6" rx="2" />
        <rect x="4" y="14" width="16" height="6" rx="2" /><path d="M8 6.5h.01M8 17.5h.01M13 6.5h4M13 17.5h4" />
      </svg>
    </span>}
    <div className="identity">
      <h3>{machine ? record.display_name || record.host : record.name}</h3>
      <p>{machine ? [record.model, record.os].filter(Boolean).join(' · ') : 'Hosted on ' + record.host}</p>
    </div>
    <div className="health-block">
      <span className={'badge ' + status}>{machine ? machineHealth[status] : status[0].toUpperCase() + status.slice(1)}</span>
      <p className="check-age"><ReadingTime stamp={checked} now={now} /></p>
    </div>
  </header>;
}

function BackupDetails({ record, now }: { record: InventoryRecord; now: number }) {
  const backup = record.config_backup;
  const label = backup?.status === 'scheduled' ? 'Scheduled' : backup?.status === 'manual' ? 'Manual copy' : 'Not recorded';
  return <Details identity={record.id + ':backup'} css="backup" summary={'Configuration backup · ' + label}>
    {backup?.locations?.length ? backup.locations.map(place => <p className="backup-location" key={place.host + '/' + place.path}>
      {place.host} / {place.path}
    </p>) : <p>No configuration backup location recorded.</p>}
    {backup?.notes && <p>{backup.notes}</p>}
    <p>{record.health?.evidence || 'No health evidence.'}</p>
    <p>Last health check: {record.health?.checked ? <ReadingTime stamp={record.health.checked} now={now} absolute /> : 'Not recorded'}</p>
    {health(record, now) === 'stale' && <p>The last report expired. Current health is unverified.</p>}
  </Details>;
}

function HostInventory({ id, inventory, now }: { id: string; inventory: InventorySnapshot; now: number }) {
  const report = inventory.inventory_accuracy;
  const host = report?.hosts.find(item => item.machine_id === id);
  if (!host || !report) return null;
  if (host.status === 'not_applicable') {
    return <div className="host-inventory"><strong>Guest inventory outside scope</strong> · {host.reason}</div>;
  }
  const verified = host.status === 'verified' && (host.expires ?? 0) > now && !inventory.error;
  const label = verified ? 'Guest inventory verified' : host.status === 'unconfigured'
    ? 'Guest inventory collector not configured' : 'Guest inventory unverified';
  const facts = host.hardware;
  const issues = report.findings.filter(item => (item.machine_id === id || item.target === id) && (item.category !== 'guest-mismatch' || verified));
  return <div className="host-inventory">
    <strong>{label}</strong>{verified && <> · <ReadingTime stamp={host.checked} now={now} /></>}
    {facts && facts.expires > now && !inventory.error ? <div>
      {facts.architecture} {facts.os} · {facts.logical_cpus} logical CPUs · {(facts.memory_bytes / 1073741824).toFixed(1)} GiB RAM ·
      Hardware checked <ReadingTime stamp={facts.checked} now={now} />
    </div> : host.status !== 'unconfigured' && <div>{facts && facts.expires <= now ? 'Hardware report expired' : 'Hardware not verified'}</div>}
    {issues.length > 0 && <Details identity={'reconciliation:' + id}
      summary={issues.length + ' inventory ' + (issues.length === 1 ? 'item' : 'items') + ' to review'}>
      <ul>{issues.map((item, index) => <li key={item.target + ':' + index}>
        {item.target}{item.guest_id ? ' / ' + item.guest_id : ''} — {item.message}{item.name ? ' (' + item.name + ')' : ''}
      </li>)}</ul>
    </Details>}
  </div>;
}

export function MachineCard({ record, inventory, now, pageVisible, announce, showServices }: {
  record: InventoryRecord; inventory: InventorySnapshot; now: number; pageVisible: boolean; announce: Announce; showServices: () => void;
}) {
  const card = useRef<HTMLElement>(null);
  const status = health(record, now);
  const metrics = record.metrics;
  const services = inventory.services.filter(service => service.machine_id === record.id).length;
  return <article ref={card} className="card machine-card" data-id={record.id}>
    <CardHeader record={record} now={now} machine />
    <HostInventory id={record.id} inventory={inventory} now={now} />
    {status !== 'stopped' && metrics?.status && metrics.status !== 'unconfigured' &&
      <Telemetry machine={record.id} host={record.host} card={card} cached={metrics} pageVisible={pageVisible} now={now} />}
    {!!record.access?.length && <MachineAccess record={record} announce={announce} />}
    <div className="card-foot"><button type="button" className="text-button" onClick={showServices}>{services} services on this machine →</button></div>
    <BackupDetails record={record} now={now} />
  </article>;
}

export function ServiceCard({ record, now, announce, showHost }: {
  record: InventoryRecord; now: number; announce: Announce; showHost: () => void;
}) {
  const endpoints = serviceEndpoints(record.endpoints ?? {});
  return <article className="card service-card" data-id={record.id}>
    <CardHeader record={record} now={now} machine={false} />
    <div className="service-body">
      {record.deployment && <p className="service-description">{record.deployment}</p>}
      <div className="service-links">{endpoints.length ? endpoints.map(endpoint => endpoint.href
        ? <ExternalLink key={endpoint.label} href={endpoint.href} css="button secondary">Open {endpoint.label} ↗</ExternalLink>
        : <p key={endpoint.label} className="no-endpoint">{endpoint.label} is local to {record.host}</p>)
        : <p className="no-endpoint">No browser endpoint registered.</p>}</div>
      {record.access?.map(route => <div className="credential-line" key={route.id}>
        <Username label="Username" route={route} />
        {route.password_available && <CopyPassword record={record} route={route} announce={announce} />}
      </div>)}
      <div className="capabilities">{record.capabilities?.map(tag => <span className="capability" key={tag}>{tag}</span>)}</div>
    </div>
    <div className="card-foot"><button type="button" className="text-button" onClick={showHost}>View {record.host} →</button></div>
    {record.kind === 'backup' ? <p className="service-body">{record.health?.evidence || 'No backup result recorded.'}</p>
      : <BackupDetails record={record} now={now} />}
  </article>;
}
