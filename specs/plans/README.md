# Home-lab plans

This index covers the public platform plans. Active plans include staged work
and the follow-up needed to finish it. The archive records verified deliveries.
Private operations planning stays outside the public index.

## Active

| Plan | Current state |
| --- | --- |
| [0001 Renewable test environments](0001-renewable-test-environments.md) | Staging verified Linux and one Tart/Xcode recipe, standalone scoped access, and Linux expiry and recovery. Cross-runtime acceptance, controller restore and managed CUDA remain open. |
| [0004 Platform source consolidation](0004-platform-source-consolidation.md) | Scoped backup collector shipped. Reusable Apple tooling, device tooling and remaining exporters still need source review. |
| [0005 Inventory coverage and change delivery](0005-inventory-coverage-and-change-delivery.md) | Catalog changes, scoped presence and one clone registration passed bounded staging checks. Broader coverage, recovery and rollout remain open. |
| [0006 Dashboard health review](0006-dashboard-health-review.md) | Eight corrections implemented and verified in disposable staging. Production promotion is pending. |
| [0007 React, TypeScript and Rust port](0007-react-typescript-and-rust-port.md) | Dashboard front end first, followed by the dashboard server, registry and allocator. |

## Archived

| Plan | Delivery |
| --- | --- |
| [0003 Inventory reconciliation](archive/0003-inventory-reconciliation.md) | Shipped authenticated guest streams, exact deployment matching, discrepancy reporting and the manual comparison. |
