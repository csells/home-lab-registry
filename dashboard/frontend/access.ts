import type { AccessRoute } from './inventory.ts';

function safeUrl(value: string | undefined, protocols: string[]): URL | null {
  try {
    const url = new URL(value ?? '');
    return protocols.includes(url.protocol) && !url.password ? url : null;
  } catch {
    return null;
  }
}

export interface MachineRoute {
  access: AccessRoute;
  href: string;
  host: string;
  ssh: boolean;
  password: boolean;
}

export function machineRoutes(routes: AccessRoute[]) {
  const available: MachineRoute[] = [];
  const unavailable: AccessRoute[] = [];
  for (const access of routes) {
    const url = safeUrl(access.url, ['http:', 'https:', 'ssh:']);
    if (!url) {
      unavailable.push(access);
      continue;
    }
    const ssh = url.protocol === 'ssh:';
    const terminal = access.browser_terminal ?? '';
    if (ssh && !/^\/terminal\/[a-z][a-z0-9-]{0,48}\/?$/.test(terminal)) {
      unavailable.push({ ...access, note: 'Browser terminal not configured.' });
      continue;
    }
    available.push({ access, ssh, href: ssh ? terminal : url.href, host: url.host,
      password: !ssh && access.password_available === true });
  }
  return { available, unavailable };
}

export function serviceEndpoints(values: Record<string, string>) {
  return Object.entries(values).flatMap(([label, value]) => {
    const url = safeUrl(value, ['http:', 'https:']);
    if (!url) return [];
    const local = ['localhost', '127.0.0.1', '[::1]'].includes(url.hostname);
    return [{ label, href: local ? null : url.href }];
  });
}
