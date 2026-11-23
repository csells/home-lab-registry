import { useState } from 'react';
import type { ReactNode } from 'react';
import { machineRoutes } from './access.ts';
import type { MachineRoute } from './access.ts';
import type { AccessRoute, InventoryRecord } from './inventory.ts';

const opened = new Set<string>();
const feedback = new Map<string, string>();
const copyLabels = ['Copy password', 'Password copied', 'Copying…', 'Copy failed'];

export function Details({ identity, css, summary, children }: {
  identity: string; css?: string; summary: string; children: ReactNode;
}) {
  const [open, setOpen] = useState(() => opened.has(identity));
  return <details className={css} open={open} onToggle={event => {
    if (event.currentTarget.open) opened.add(identity);
    else opened.delete(identity);
    setOpen(event.currentTarget.open);
  }}><summary>{summary}</summary>{children}</details>;
}

export function ExternalLink({ href, children, css = 'button primary', label }: {
  href: string; children: ReactNode; css?: string; label?: string;
}) {
  return <a href={href} className={css} target="_blank" rel="noopener noreferrer" aria-label={label}>{children}</a>;
}

export function Username({ label, route }: { label: string; route?: Pick<AccessRoute, 'username' | 'username_required'> }) {
  return <div className="username">
    <span className="username-label">{label}</span>
    <div className={'username-value' + (route?.username ? '' : ' no-username')}>
      {route?.username || (route?.username_required === false ? 'Not required' : 'Not recorded')}
    </div>
  </div>;
}

type Announce = (message: string) => void;

export function CopyPassword({ record, route, announce }: {
  record: InventoryRecord; route: Pick<AccessRoute, 'id' | 'label'>; announce: Announce;
}) {
  const key = record.id + ':' + route.id;
  const [message, setMessage] = useState(feedback.get(key) || 'Copy password');
  function update(value: string) {
    feedback.set(key, value);
    setMessage(value);
    announce(value);
  }
  async function copy() {
    update('Copying…');
    try {
      if (!navigator.clipboard || !window.ClipboardItem) throw new Error('Clipboard unsupported');
      const value = fetch('api/copy-password', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', 'X-Lab-Action': 'copy-password' },
        body: JSON.stringify({ machine: record.id, login: route.id }),
        cache: 'no-store',
        signal: AbortSignal.timeout(15000),
      }).then(response => {
        if (!response.ok) throw new Error('Copy failed');
        return response.text();
      }).then(text => new Blob([text], { type: 'text/plain' }));
      // The clipboard API must be invoked during the click, before the HTTP response arrives.
      await navigator.clipboard.write([new ClipboardItem({ 'text/plain': value })]);
      update('Password copied');
    } catch {
      update('Could not copy. Check clipboard permission and retry.');
    }
  }
  return <button type="button" className="button secondary password-copy" disabled={message === 'Copying…'} title={message}
    aria-label={'Copy ' + (record.display_name || record.host) + ' ' + route.label + ' password'}
    onClick={() => void copy()}>
    {copyLabels.map(label => <span key={label} className="copy-size" aria-hidden="true">{label}</span>)}
    <span className="copy-label">{message.startsWith('Could not copy') ? 'Copy failed' : message}</span>
  </button>;
}

const routeTitles: Record<string, string> = { kvm: 'Network KVM', 'amt-off': 'Intel AMT · OS off', amt: 'Intel AMT' };
const actions: Record<string, string> = { proxmox: 'Open Proxmox', kvm: 'Open KVM', console: 'Open terminal login' };

function RouteCard({ record, route, announce }: { record: InventoryRecord; route: MachineRoute; announce: Announce }) {
  const { access, ssh, href, host, password } = route;
  const consoleLogin = record.console_login;
  const device = Object.hasOwn(routeTitles, access.id);
  const title = ssh ? 'SSH terminal' : routeTitles[access.id] || access.label;
  const label = ssh ? 'Open SSH terminal' : actions[access.id] || 'Open Intel AMT';
  const destination = ssh ? record.display_name || record.host : record.host + (access.id === 'amt-off' ? ' · OS off' : '');
  return <section className="access-row" data-login={access.id}>
    <div className="route-head"><h4>{title}</h4><span className="route-kind">{ssh ? 'BROWSER' : 'LAN / VPN'}</span></div>
    <p className="login-address">{host}</p>
    <div className="credential-line">
      <Username label={device ? 'Device username' : ssh ? 'Connects as' : 'Username'} route={access} />
      {password && <CopyPassword record={record} route={access} announce={announce} />}
    </div>
    {device && <div className="credential-line console-credentials">
      <Username label="Host console username" route={consoleLogin ?? undefined} />
      {consoleLogin?.password_available
        ? <CopyPassword record={record} route={{ id: consoleLogin.credential_id, label: 'host console' }} announce={announce} />
        : <span className="console-unavailable">Password not recorded</span>}
    </div>}
    <div className="login-actions"><ExternalLink href={href} label={label + ' · ' + destination}>{label} ↗</ExternalLink></div>
    <Details identity={record.id + ':route:' + access.id} css="route-details" summary="Login details">
      <p>{ssh ? 'This terminal runs in your browser and connects through the lab. You are logged in automatically; no SSH password or installed app is required.'
        : 'This opens the device’s own login page. You need home-network or VPN access.'}</p>
      {access.note && <p>{access.note}</p>}
      {!ssh && !password && <p>{access.password_note || 'No password stored for this login.'}</p>}
    </Details>
  </section>;
}

export function MachineAccess({ record, announce }: { record: InventoryRecord; announce: Announce }) {
  const { available, unavailable } = machineRoutes(record.access ?? []);
  return <section className="access-panel" aria-label={record.host + ' login paths'}>
    <div className="section-kicker">CONNECT<span className="plain">Your ways into {record.display_name || record.host}</span></div>
    {available.length ? <>
      <div className="access-grid">{available.map(route => <RouteCard key={route.access.id} record={record} route={route} announce={announce} />)}</div>
      <p className="access-hint">Browser SSH works from anywhere. Device login pages marked LAN / VPN require access to your home network.</p>
    </> : <p className="empty-access">No verified login paths yet. Access details need to be registered before this machine can be opened.</p>}
    {unavailable.length > 0 && <Details identity={record.id + ':unavailable'} css="unavailable-paths"
      summary={unavailable.length + ' unavailable ' + (unavailable.length === 1 ? 'path' : 'paths')}>
      {unavailable.map(route => <p key={route.id}>{route.label} — {route.note || 'Not configured.'}</p>)}
    </Details>}
  </section>;
}
