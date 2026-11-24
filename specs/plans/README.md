# Home-lab plans

This index covers the public platform plans. Active plans track implementation
and verification work. The archive records verified deliveries. Private
operations planning stays outside the public index.

## Active

| Plan | Current state |
| --- | --- |
| [0001 Renewable test environments](0001-renewable-test-environments.md) | Production verified the Proxmox and Tart lease lifecycle and operator email delivery. Checkpoint and disaster recovery, agent onboarding, delegated renewal, worker maintenance, shared services and recipe qualification remain open. |
| [0004 Platform source consolidation](0004-platform-source-consolidation.md) | The backup collector and allocator run in production. Reusable Mac tooling packaging and additional component provenance remain open. |
| [0005 Inventory coverage and change delivery](0005-inventory-coverage-and-change-delivery.md) | Production verified a connected catalog feed, six guest collectors and fresh hardware from five publishers. Event, presence and deployment-tool coverage remain open. |
| [0009 Production platform verification](0009-production-platform-verification.md) | Tracks dashboard session recovery, live discrepancy and lease-hold verification, a full-platform performance baseline and deferred drills. |

## Archived

| Plan | Delivery |
| --- | --- |
| [0003 Inventory reconciliation](archive/0003-inventory-reconciliation.md) | Shipped authenticated guest streams, exact deployment matching, discrepancy reporting and the manual comparison. |
| [0006 Dashboard health review](archive/0006-dashboard-health-review.md) | Eight dashboard corrections verified in staging. Plan 0009 carries remaining production checks. |
| [0007 React, TypeScript and Rust port](archive/0007-react-typescript-and-rust-port.md) | Core port verified in disposable staging. Plan 0008 records production delivery and plan 0009 tracks follow-up. |
| [0008 Finish the Rust and TypeScript migration](archive/0008-remove-python-and-javascript.md) | Shipped the combined registry, dashboard and allocator release and the Rust fleet backup hook. Plan 0009 carries remaining verification. |
