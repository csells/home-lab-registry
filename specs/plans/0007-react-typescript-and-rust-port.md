# React, TypeScript and Rust port

Status — active. Start from the dashboard health review candidate in
[Plan 0006](0006-dashboard-health-review.md). Keep existing home-lab clients,
the browser workflow, and persisted state compatible through the port.

## Order of delivery

| Slice | Acceptance |
| --- | --- |
| React and TypeScript live state and Discrepancies | Existing routes, filters, counts and layout carry over. Startup reads, live events, refresh results and reconciliation receipts share one typed state model. Only the accepted receipt settles a comparison. An older snapshot cannot erase a newer one. Verify the same browser workflows while streams reconnect and with SSE disabled. |
| React and TypeScript views | Port Machines, Services, Backups, Resources and Leases. Retain chart subscriptions, graph behavior, lease paging, accessibility, and scoped login actions. Remove the replaced browser code after behavior checks pass. |
| Rust dashboard server | Match the existing dashboard HTTP and event contracts. Verify authentication, origins, terminal access, collector identity, backpressure, reconnect and shutdown on the full staging stack. |
| Rust registry | Preserve catalog revisions, enrollment and health report semantics, scoped authorization, SQLite compatibility and audit history. Test a backup restore and exercise Python and Rust-compatible clients through the existing interfaces. |
| Rust lease allocator | Preserve lease ownership, durable operation state, checkpoint receipts, provider boundaries, recovery and expiry. Exercise open-lease recovery, restore and release through the existing clients and real staged dependencies before migration. |

Go remains the on-host collector language. Keep provider tools and scripts in
their current language where the runtime interface is stable. Choose Rust
libraries when their slice starts and keep transport contracts independent of
the implementation.

## Delivery rules

Write failing behavior tests before each change. Keep the browser bundle a
same-origin static asset. Test each candidate against real lab interfaces in a
disposable staging deployment. Preserve separate release and rollback artifacts
and keep production approvals explicit for each cutover. Measure idle work,
stream behavior, and recovery against the prior candidate. Any intended behavior
change requires its own test and rationale.

## First staging delivery

Revision `3359475` ports the shared dashboard inventory state, Discrepancies,
and Leases to React and TypeScript. It keeps the established Python API and
same-origin authentication. The remaining views still render through the
existing JavaScript and use the shared inventory state during the transition.

Seven TypeScript behavior tests cover event ordering, comparison ownership,
stream loss, invalid snapshots, and lease page ordering. The browser fixture
passed at phone and desktop widths, including lease pagination. The full
disposable preview verified six live collectors, a real health failure and
recovery, reconciliation through the matching receipt with browser SSE blocked,
stream recovery, and navigation through all dashboard tabs. The built bundle
was verified over authenticated HTTP with the same content security policy.
