import { createRoot } from 'react-dom/client';
import { App } from './app-view.tsx';
import { DashboardInventory } from './inventory.ts';
import { LeaseList } from './leases.ts';

if (location.pathname === '/leases') history.replaceState(null, '', '/' + location.search + '#leases');

const store = new DashboardInventory();
const leases = new LeaseList();
const root = document.getElementById('dashboard');

if (root) createRoot(root).render(<App store={store} leases={leases} />);
