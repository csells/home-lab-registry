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


## Coverage and cutover qualification

The revised candidate separates confirmed guest mismatches, records needing
review, and unverified collectors. The manual comparison reports verified
coverage and the oldest verified observation. Machine cards show the
collector state. The API reports coverage independently from findings and
tags each finding with its category.

A real mobile browser verified partial coverage, the grouped report and
manual comparison. Stopping the dedicated staging collector dropped the
verified count to zero. Restarting it restored coverage in the open browser.
The production collectors responded to authenticated snapshot requests but
returned 404 on the new stream route. Fleet streaming remains a cutover check.

After approval, capture and verify a recoverable pre-cutover configuration
backup. Preserve the existing dashboard files and each installed collector
binary, with digests and service definitions, in the approved rollback path.
Build the candidate for each enrolled OS and architecture. Upgrade collectors
one at a time and verify both existing metrics and guest snapshots as well as
a real authenticated stream before proceeding to the next host. Restore the
previous binary and verify its endpoints if a host regresses. Deploy the
dashboard after the collectors pass. If dashboard verification fails, restore
its previous files and verify the stable views and access paths. Confirm fleet
coverage and authenticated browser behavior before removing staging.


## Production rollout (2026-11-20)

Approved implementation `51631f7` is live. The installed collectors served
authenticated metrics, guest snapshots and guest streams for all six configured
hosts. The production dashboard returned 6/6 through SSE and after a manual
comparison. An authenticated mobile browser rendered the coverage panel, host
cards and Resources graph. Previous binaries and dashboard code were retained
outside staging and checked for readability. The dedicated staging VM and disks,
audit account and review route were removed.

Remaining follow-up: registry metadata change delivery, OS-native lifecycle
events, native/cloud presence discovery and deployment-tool registration.


## Catalog metadata change candidate

The registry can send a bounded, reader-authenticated change stream. A committed
catalog edit wakes the dashboard, which reloads current registry records.
Heartbeats carry no catalog data and do not reload it. Connection and reconnection
invalidate the dashboard cache so a missed change does not require event replay.
Health checks and machine presence do not trigger metadata reloads.

Configure `catalog_change_stream: true` in the dashboard reader configuration
after its registry exposes `GET /v1/catalog/changes` on the trusted HTTPS origin.
The feature defaults off for older registry versions. Manual reconciliation
continues to work independently.

A run in the disposable staging VM used an isolated copy of real catalog records
and the actual lab collector streams. Retiring the copy of an existing staging
record updated the candidate dashboard without a manual refresh. All six
configured collectors remained verified. Staging writes used a separately
generated credential and stayed in the isolated catalog.

## Native and externally managed deployment observations

A service can bind to an exact native or external deployment through
`resources.runtime_binding` with `kind`, `provider`, and `instance`.
The operator enrolls an observer for that exact binding in the candidate
registry's `runtime_publishers` configuration. Its credential permits posting
only to the bound record. A changed or retired binding invalidates the observed
state and rejects reports from the previous enrollment.

A trusted local observer can use `registry/publish_runtime.py CONFIG --systemd`
with an exact unit. An external provider adapter uses the same publisher with
`--state` after verifying the provider-side identity. An observer reports
`missing` only for an explicit absence. Provider failure, denied access and stale
data are unverified. Runtime presence is separate from functional service health.
The registry pushes bounded freshness updates alongside catalog changes.

## Deployment-tool registration

New deployment tooling should call `registry/register_deployment.py` with the
trusted registry client and a manifest after confirming the provider identity.
The manifest declares the owner, exact runtime binding, and immutable
`resources.deployment_generation`. A retry can verify the same active
generation after an interrupted acknowledgment. A different generation or
retired record cannot be overwritten. After checking provider teardown, the
same tool retires the matching generation and verifies the registry readback.
The helper has no provider-mutation permissions.

The candidate exercised registration retry and wrong-generation retirement
against an actual disposable Proxmox clone. Runtime publishers used the real
systemd service and authenticated Eero network identity. The staging dashboard
updated from the catalog stream, with all six configured guest collectors
verified. The native observation route rejected updates after the deployment
binding changed.

## Native Proxmox wake events

On a Linux Proxmox collector, optionally configure
`guests.task_events_directory` to the local Proxmox task directory.
Linux inotify invalidates the guest snapshot when Proxmox updates active tasks
or the completed task index. The collector reads the authenticated Proxmox API
to determine guest state and coalesces rapid task updates. The local
`interval_seconds` sample remains a freshness fallback for unavailable or
unsupported event sources. The dashboard does not poll the fleet.

A temporary local collector on the actual Proxmox host observed a disposable
guest's start and removal before its five-minute fallback sample. The candidate
did not replace the installed collector.

## Native Tart wake events

Set `guests.tart_watch_events: true` on a macOS collector running as the
Tart owner. It watches the local inventory and VM directories. Tart creates
its control socket when starting a VM. When Tart's configuration lock
identifies a running process, the collector also watches that process exit.
Both signals refresh the cached inventory through `tart list`. The local
sample preserves freshness when a path or process watch is unavailable.

A separate local collector on the actual Mac streamed clone creation, start,
stop, and deletion using Tart 2.39.0. All four updates arrived before its
five-minute fallback sample. No installed collector was replaced.
