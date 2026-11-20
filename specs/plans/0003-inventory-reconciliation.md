# Inventory reconciliation

## Outcome

People and agents can distinguish registered intent from freshly observed deployments,
see discrepancies, and request a new deterministic reconciliation without an AI session.

## Implementation

- Reuse existing authenticated metrics-agent guest inventory; preserve guest identity,
  type, name, template status, observation time and expiry.
- Join deployments through explicit host/guest bindings, never prose or IP guesses.
  Support existing structured guest IDs during migration.
- Report missing and unregistered guests, expired leases, missing deployment metadata,
  duplicate machine identities and unavailable collectors separately from service health.
  Multiple services may legitimately share a guest.
- Display discovered guest inventory as the current view. Historical snapshots are not
  runtime truth. Preserve history; do not rewrite or delete deployments automatically.
- Subscribe to collector inventory streams and send dashboard updates through SSE.
  Use cached snapshots on connect, content-change events and freshness heartbeats.
  No recurring central fleet polling or snapshot fallback. Manual Reconcile now
  reloads metadata and reconnects to cached collector reports.
- Document registration and verified retirement contracts for deployment agents.

## Verification and rollout

Run the candidate in a disposable VM against the real registry and real mTLS collectors,
using read-only registry access and a dedicated short-lived client certificate. Validate
actual browser controls and compare findings with the hypervisor inventories. Exercise
freshness and failure semantics without changing live workloads. Preserve private evidence
outside Git. Stable promotion requires review and explicit approval; cleanup follows it.

## Boundaries

This does not introduce an allocator, infer service purpose from process names, or
silently retire missing resources. Native/cloud application discovery and physical
hardware refresh need their own verified sources; report their coverage honestly.

## Candidate validation (2026-11-06)

31 dashboard unit tests pass in the disposable VM. The running candidate reads the
real registry and all six configured collectors; their guest identities and states
matched direct Proxmox/Tart queries. Browser interaction verified manual reconciliation,
mobile layout, the report API and rejection of cross-origin mutation requests.
The comparison initially failed under sandbox-limited macOS process visibility;
a direct permitted host query resolved that discrepancy without changing workloads.

The initial snapshot candidate was superseded by the streaming candidate below.
Native/cloud presence discovery and mandatory deployment-tool registration remain
separate follow-up work.

## Push correction

The earlier 30-second dashboard fleet poll was rejected and removed. The candidate
now uses a dedicated collector inventory stream and browser SSE. Local hypervisor
sampling remains unchanged; OS-native event discovery and registry metadata-change
streaming are explicitly outstanding. Stable collectors need a reviewed upgrade;
validation uses a candidate collector reading the real hypervisor with a dedicated
read-only token, not a synthetic hypervisor. Earlier six-collector snapshot validation
does not establish that the new streaming path is deployed fleet-wide.

Push-path validation passed: 35 Python tests, all Go tests and the stream race test;
actual guest-name change and restoration propagated from the real hypervisor to the
browser without another inventory GET. A 65-second idle/heartbeat window retained
one browser subscription with no polling, followed by successful manual reconciliation.
The runtime source samples at its existing interval; this is change-only network
delivery, not yet an OS-native hypervisor lifecycle event source.


## Integrated staging review (2026-11-20)

The staged build combines the Resources graph with inventory streaming and the
already deployed operator-run address audit. It connects to the real registry
and one of six configured collectors through the staging stream endpoint.
The other five remain unverified in this candidate. The browser rendered the
graph and completed a manual reconciliation with one inventory GET during
the session. Dashboard (37 tests), registry (20 tests), Go race and graph
tests passed. The review endpoint and expiry are recorded in the registry.

Production promotion requires approval of this integrated build. After
approval, preserve rollback evidence, upgrade enrolled collectors, verify
coverage and authenticated production behavior, then remove the staging
guest, routes and dedicated credentials. Confirm the deployed source in
the registry and update the architecture after verification.
