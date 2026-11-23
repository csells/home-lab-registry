import type { DashboardInventory } from './inventory.ts';

export async function readInventory(store: DashboardInventory, signal?: AbortSignal) {
  const timeout = AbortSignal.timeout(12000);
  try {
    const response = await fetch('api/inventory', { signal: signal ? AbortSignal.any([signal, timeout]) : timeout, cache: 'no-store' });
    if (!response.ok) throw new Error('HTTP ' + response.status);
    store.apply(await response.json());
  } catch {
    if (!signal?.aborted) store.requestFailed('Unable to refresh. Displayed readings may be stale.');
  }
}

export async function refreshCatalog(store: DashboardInventory) {
  try {
    const response = await fetch('api/refresh', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', 'X-Lab-Action': 'refresh-catalog' },
      body: '{}',
      signal: AbortSignal.timeout(16000),
    });
    if (response.status === 403) {
      await readInventory(store);
      return;
    }
    if (!response.ok) throw new Error('HTTP ' + response.status);
    store.apply(await response.json());
  } catch (error) {
    store.requestFailed('Catalog refresh failed: ' + (error instanceof Error ? error.message : 'request failed'));
  }
}

export function connectDashboard(store: DashboardInventory, setVisible: (visible: boolean) => void) {
  let source: EventSource | undefined;
  let retry: ReturnType<typeof setTimeout> | undefined;
  let request: AbortController | undefined;
  let delay = 1000;
  let pageHidden = false;
  const visible = () => !document.hidden && !pageHidden;

  function disconnect() {
    clearTimeout(retry);
    source?.close();
    source = undefined;
  }

  function connect() {
    disconnect();
    if (!visible()) return;
    const stream = new EventSource('api/inventory/stream');
    source = stream;
    stream.addEventListener('inventory', event => {
      if (source !== stream) return;
      try {
        store.apply(JSON.parse(event.data), 'stream');
        delay = 1000;
      } catch {
        store.streamUnavailable('Invalid inventory event; waiting for a valid update.');
      }
    });
    stream.onerror = () => {
      if (source !== stream) return;
      store.streamUnavailable('Inventory stream disconnected; reconnecting. Cached observations may be stale.');
      if (stream.readyState === EventSource.CLOSED) {
        clearTimeout(retry);
        retry = setTimeout(connect, delay);
        delay = Math.min(30000, delay * 2);
      }
    };
  }

  function visibilityChanged() {
    const shown = visible();
    setVisible(shown);
    request?.abort();
    if (shown) {
      request = new AbortController();
      void readInventory(store, request.signal);
      connect();
    } else {
      disconnect();
    }
  }

  function pagehide() {
    pageHidden = true;
    visibilityChanged();
  }

  function pageshow(event: PageTransitionEvent) {
    if (!event.persisted && !pageHidden) return;
    pageHidden = false;
    visibilityChanged();
  }

  function online() {
    if (source?.readyState !== EventSource.OPEN) connect();
  }

  document.addEventListener('visibilitychange', visibilityChanged);
  window.addEventListener('pagehide', pagehide);
  window.addEventListener('pageshow', pageshow);
  window.addEventListener('online', online);
  const tick = setInterval(() => {
    if (visible()) store.tick();
  }, 15000);
  visibilityChanged();

  return () => {
    document.removeEventListener('visibilitychange', visibilityChanged);
    window.removeEventListener('pagehide', pagehide);
    window.removeEventListener('pageshow', pageshow);
    window.removeEventListener('online', online);
    clearInterval(tick);
    request?.abort();
    disconnect();
  };
}
