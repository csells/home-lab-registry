# Home-lab plans

This index covers the public platform plans. Active plans include staged work
and the follow-up needed to finish it. The archive records verified deliveries.
Private operations planning stays outside the public index.

## Active

| Plan | Current state |
| --- | --- |
| [0001 Renewable test environments](0001-renewable-test-environments.md) | Staging verified Codex and Pi recovery, fenced controller replacement, three concurrent Proxmox leases, managed CUDA and Metal, active disk growth and scoped service grants. Broader operating policy and recovery acceptance remain. |
| [0004 Platform source consolidation](0004-platform-source-consolidation.md) | Backup collector shipped. Apple rebuild and device and exporter provenance have evidence. Remaining component source and recovery review continues. |
| [0005 Inventory coverage and change delivery](0005-inventory-coverage-and-change-delivery.md) | Staging verified native event fallback, hardware on five eligible hosts and native deployment registration. Installed publisher and deployment-tool coverage remain open. |
| [0008 Finish the Rust and TypeScript migration](0008-remove-python-and-javascript.md) | Registry, dashboard and fleet backup hooks run in production. Allocator authority stays in staging pending production enrollment. |

## Archived

| Plan | Delivery |
| --- | --- |
| [0003 Inventory reconciliation](archive/0003-inventory-reconciliation.md) | Shipped authenticated guest streams, exact deployment matching, discrepancy reporting and the manual comparison. |
| [0006 Dashboard health review](archive/0006-dashboard-health-review.md) | Eight dashboard corrections verified in staging. Plan 0008 carries the integrated production checks. |
| [0007 React, TypeScript and Rust port](archive/0007-react-typescript-and-rust-port.md) | Core port verified in disposable staging. Plan 0008 carries the final contract checks and rollback. |
