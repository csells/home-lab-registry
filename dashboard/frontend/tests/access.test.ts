import assert from 'node:assert/strict';
import { test } from 'node:test';
import { machineRoutes, serviceEndpoints } from '../access.ts';

test('SSH opens only its scoped in-browser terminal and never exposes a copy-password action', () => {
  const routes = machineRoutes([
    { id: 'ssh', label: 'Shell', url: 'ssh://worker.local:22', browser_terminal: '/terminal/test-worker/', password_available: true },
    { id: 'ssh-other', label: 'Other shell', url: 'ssh://other.local', browser_terminal: '/terminal/../api/inventory' },
    { id: 'kvm', label: 'KVM', url: 'https://console.local', password_available: true },
  ]);
  assert.deepEqual(routes.available.map(route => [route.access.id, route.href, route.password]), [
    ['ssh', '/terminal/test-worker/', false], ['kvm', 'https://console.local/', true],
  ]);
  assert.match(routes.unavailable[0].note ?? '', /Browser terminal/);
});

test('localhost service addresses are descriptions and registered HTTPS URLs are links', () => {
  assert.deepEqual(serviceEndpoints({ loopback: 'http://127.0.0.1:9000', dashboard: 'https://example.invalid/' }), [
    { label: 'loopback', href: null }, { label: 'dashboard', href: 'https://example.invalid/' },
  ]);
});
