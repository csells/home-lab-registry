# React, TypeScript and Rust port

Status: finished in staging with a remainder, reconciled 2026-11-23.
The core port passed disposable staging at `0230fe1`. That revision is the
executable comparison baseline. [Plan 0008](../0008-remove-python-and-javascript.md#carried-from-archived-plans)
owns the remaining integrated and production contract checks and rollback.
The port builds on the [dashboard health review](0006-dashboard-health-review.md).

## Order of delivery

| Slice | Acceptance |
| --- | --- |
| React and TypeScript live state and Discrepancies | Existing routes, filters, counts and layout carry over. Startup reads, live events, refresh results and reconciliation receipts share one typed state model. Only the accepted receipt settles a comparison. An older snapshot cannot erase a newer one. Verify the same browser workflows while streams reconnect and with SSE disabled. |
| React and TypeScript views | Port Machines, Services, Backups, Resources and Leases. Retain chart subscriptions, graph behavior, lease paging, accessibility, and scoped login actions. Remove the replaced browser code after behavior checks pass. |
| Rust dashboard server | Match the existing dashboard HTTP and event contracts. Verify authentication, origins, terminal access, collector identity, backpressure, reconnect and shutdown on the full staging stack. |
| Rust registry | Preserve catalog revisions, enrollment and health report semantics, scoped authorization, SQLite compatibility and audit history. Test a backup restore and exercise Python and Rust-compatible clients through the existing interfaces. |
| Rust lease allocator | Preserve lease ownership, durable operation state, checkpoint receipts, provider boundaries, recovery and expiry. Exercise open-lease recovery, restore and release through the existing clients and real staged dependencies before migration. |

This milestone kept provider tools and scripts behind stable interfaces.
[Plan 0008](../0008-remove-python-and-javascript.md) subsequently ported those
first-party tools. Go remains the on-host collector language.

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

## Rust allocator staging

Revision `421b062` serves the existing task, review, and worker APIs over TLS. Rust owns admission, lease state, durable provider intent, and checkpoint
receipts in the existing SQLite ledger. The independent witness still fences
mutations after a stale database restore or whole-controller rollback.
Proxmox and Tart operations use the pinned provider tools. Existing workers
still capture and recover the data through their scoped storage credentials.

Native tests cover task credential issuance and revocation, owner isolation,
request replay, witness fencing, unique recovery of an unknown provider task,
renewals and holds, and exact storage receipts. The legacy task API schema and
the lease review responses from a consistent copy of the actual staging
database matched.

On the disposable controller, the existing client acquired and resized a
real Proxmox clone, renewed the lease, restarted the controller, and replayed
the same request. A second principal was refused access. The task exported
and recovered its incremental checkpoint, pinned and unpinned it, and restored
the final checkpoint into a matching replacement with a fresh key. A separate
fixed-slot lease expired under a 15-second staging grace, stored its workspace,
and reset automatically. The mobile dashboard showed the live lease.

A single Tart lease survived a controller restart and returned verified SSH
access. Xcode ran and listed available iPad simulators. The worker then
exported the declared workspace, the client downloaded and verified its bytes
after release, and the worker confirmed that the owned clone and workspace
were gone.

The preview returned to its usual grace period, and the three test task
credentials were revoked. The existing Python allocator started against the
same idle ledger during the rollback check. Rust then resumed service, with
matching lease review counts and history and no active leases or restore
holds. During two 18-second idle samples, Python used 34.0 MiB RSS and 0.11%
of one core, and Rust used 8.7 MiB RSS with no measured CPU ticks. These brief
samples exclude guest and export work.
