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

## Full React dashboard

Revision `2be020c` serves all dashboard views from React and TypeScript. The
last browser script was removed. ELK still handles graph layout within the
Resources view. Metrics streams follow card visibility and close when the
Machines view or the browser page is hidden. The shared inventory state keeps
explicit lease availability even if the initial HTTP response arrives after an
inventory stream event.

The disposable preview verified all tabs, six live collectors, a real service
health failure and recovery, the receipt for a comparison with browser SSE
blocked, a live metrics chart, and the staging-only terminal and password
controls. Browser fixtures covered phone and desktop layout, history paging,
the graph, GPU reading states, and closing and reopening chart streams.

## Rust dashboard and catalog staging

The disposable preview serves the Rust dashboard from revision `4f6d9bd` and
the Rust registry from revision `4c4d65e`. The dashboard matched the Python
backend's inventory and discrepancy identities across six collectors. Browser
checks verified every tab, live chart readings, masked clipboard copying, the
disposable Guacamole route, a service failure and recovery, and a matching
reconciliation receipt while browser SSE was blocked. HTTP checks verified
refresh, bounded stream state, and graceful shutdown.

The Rust registry served a SQLite-consistent copy of the real staging catalog
through its established TLS API. The Python client read and wrote it. Checks
covered revision conflicts, health-only change delivery, scoped runtime
publication, real collector mTLS identity, and export restored with the existing
operator tool. The dashboard then repeated the browser checks against the Rust
registry on the original staging TLS endpoint. The staged units have independent
rollback artifacts.

A 20-second comparison on the disposable VM measured 0.15% of one core and
32.8 MiB RSS for Rust, versus 0.00% and 34.7 MiB for Python. Both servers had six
idle collector subscriptions and no chart viewers. The sample is too brief to
infer a steady-state CPU difference. A Linux regression test verifies the
existing self-signed registry certificate and rejects a mismatched hostname.
Collector streams still require their own TLS 1.3 identity.

## Rust allocator work in progress

Native tests verify the existing Python SQLite ledger and independent witness,
single authority, stale database and controller generations, durable operation
intent, and unique recovery of an otherwise unknown provider task. They also
cover lease review and retained-checkpoint availability, owner and reviewer
holds, renewal, and the requirement for verified export before reclamation.
The existing allocator still serves the preview while the remaining controller
routes, provider transitions and recovery are ported and tested.
