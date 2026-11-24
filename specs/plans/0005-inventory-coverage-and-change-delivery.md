# Inventory coverage and change delivery

Status: active, reconciled 2026-11-23. Production has a connected registry
change feed, six verified guest streams, fresh hardware from five installed
publishers and a verified manual comparison. The remaining work extends
publisher and deployment-tool coverage and verifies event delivery and
recovery on the production access path.

## Carried from archived plans

[Plan 0003](archive/0003-inventory-reconciliation.md) shipped guest reconciliation.
Its [remaining follow-up](archive/0003-inventory-reconciliation.md#remaining-follow-up)
and [scope boundary](archive/0003-inventory-reconciliation.md#boundaries) carry forward.

| Requirement | What shipped or has evidence | Acceptance still open |
| --- | --- | --- |
| Registry metadata change delivery | The production dashboard reported a connected catalog feed. Authenticated Refresh and Reconcile now worked with all six collectors. Staging resumed from a known cursor after two disconnected writes and retried an HTTP 503 from the first refresh. The current catalog appeared in 3.55 seconds. | Exercise a benign metadata change and reconnect through the authenticated production dashboard. |
| OS-native lifecycle events | Collectors sample local Proxmox and Tart inventory and stream changes. A disposable Tart check observed create, start, stop and delete before the sampling fallback. On a real Proxmox host, the native task watcher observed a disposable start in about 21 seconds with a four-minute sampling interval. An independent collector with an unavailable watcher recovered by periodic sampling. A second Tart check observed an actual clone and deletion with native events disabled. The candidate then lost an already-open watcher on both Proxmox and Tart. The Proxmox collector produced a new polling sample after 9.4 seconds. Tart discovered a real clone about 10 seconds after the isolated store's watch was lost. The disposable dashboard received the switch from event-assisted delivery to polling over the normal authenticated collector stream, then received a fresh periodic report. | Verify event-assisted delivery and fallback on supported installed collectors. All five installed collectors that reported a delivery mode were polling in the production observation. |
| Native/cloud presence discovery | Stable verifies guest bindings from authenticated hypervisor observations. Staging verified a real systemd unit and a cloud endpoint using scoped publishers. It rejected a report for an old binding. The Rust publisher reported actual installation and removal to the staging registry, and the test verified expiry and an explicitly unavailable probe while the unit remained installed. | Verify publisher coverage for the supported deployed identities. |
| Physical hardware refresh | Five installed eligible collectors delivered fresh OS, architecture, logical CPU count and RAM capacity to the production dashboard. Independent host readings matched the temporary pre-rollout builds. Hardware observations have their own timestamps and expiry. | Confirm coverage as supported publishers are enrolled. The personal-use host is limited to its existing collector. |
| Mandatory deployment-tool registration | The [operating contract](../../docs/inventory-reconciliation.md) requires registration and verified retirement. A real staging Linux clone exercised replay, rejection of a stale generation and verified retirement after deletion. Staging also recovered lost replies to registration and retirement for a bound service without repeating either write. The native launchd tool registered a real GUI job, retained it while idle or while its plist remained installed, rejected a stale generation and retired the exact removed job. | Apply the contract in other supported deployment tools. Verify router-side attribution where relevant and provider-specific teardown. Allocator-specific ownership stays in [plan 0001](0001-renewable-test-environments.md). |

The integrated Rust review verified address rebinding and scoped retirement.
The native systemd deployment sync registered a real discovery service on a
scheduled tick, preserved its record while stopped, retired it after confirmed
removal and rejected a stale generation. Production reported six verified
collectors and three unconfigured entries. The offline desktop is outside
current qualification. Continue verifying supported publishers and deployment
tools as they are enrolled. [Plan 0009](0009-production-platform-verification.md)
tracks the remaining production dashboard session and failure-recovery checks.

Keep change delivery deterministic. Observations inform discrepancy reports.
Retirement and resource deletion require independent ownership verification
through the authority responsible for the deployment.

[Plan 0006](archive/0006-dashboard-health-review.md) records the review corrections to
manual comparison, catalog freshness and health delivery.
