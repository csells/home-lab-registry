# Inventory coverage and change delivery

Status: active, reconciled 2026-11-23. Production has streamed guest inventory and manual reconciliation.
The remaining work extends what can be observed, when records update and which
deployment tools register their work. The
[catalog change review](https://github.com/csells/home-lab-registry/tree/82dc0fa)
passed [CI](https://github.com/csells/home-lab-registry/actions/runs/44995943081)
and has bounded staging evidence. Production still uses the behavior below.

## Carried from archived plans

[Plan 0003](archive/0003-inventory-reconciliation.md) shipped guest reconciliation.
Its [remaining follow-up](archive/0003-inventory-reconciliation.md#remaining-follow-up)
and [scope boundary](archive/0003-inventory-reconciliation.md#boundaries) carry forward.

| Requirement | What shipped or has evidence | Acceptance still open |
| --- | --- | --- |
| Registry metadata change delivery | Stable reads at startup and on **Reconcile now**. Staging resumed from a known stream cursor after two disconnected writes and retried an HTTP 503 from the first refresh. The current catalog appeared in 3.55 seconds with all six configured collectors verified. The review app also completed manual reconciliation and opened Leases at mobile width. | Verify the current candidate over the authenticated public review path. |
| OS-native lifecycle events | Collectors sample local Proxmox and Tart inventory and stream changes. A disposable Tart check observed create, start, stop and delete before the sampling fallback. On a real Proxmox host, the native task watcher observed a disposable start in about 21 seconds with a four-minute sampling interval. An independent collector with an unavailable watcher recovered by periodic sampling. A second Tart check observed an actual clone and deletion with native events disabled. The candidate then lost an already-open watcher on both Proxmox and Tart. The Proxmox collector produced a new polling sample after 9.4 seconds. Tart discovered a real clone about 10 seconds after the isolated store's watch was lost. The disposable dashboard received the switch from event-assisted delivery to polling over the normal authenticated collector stream, then received a fresh periodic report. | Confirm delivery evidence from other installed collectors as they support it. |
| Native/cloud presence discovery | Stable verifies guest bindings from authenticated hypervisor observations. Staging verified a real systemd unit and a cloud endpoint using scoped publishers. It rejected a report for an old binding. The Rust publisher reported actual installation and removal to the staging registry, and the test verified expiry and an explicitly unavailable probe while the unit remained installed. | Verify publisher coverage for the supported deployed identities. |
| Physical hardware refresh | The candidate adds OS, architecture, logical CPU count and RAM capacity with their own observation timestamp over the authenticated guest stream. A temporary collector on a real Proxmox host matched local CPU and RAM readings, and the Mac passed the native sampler. The dashboard distinguishes expired and unverified facts. A live candidate dashboard showed six verified guest collectors and zero fresh hardware reports from their installed sources. Temporary candidate collectors matched independent host CPU and memory readings across all four eligible Proxmox nodes and the Mac. | Verify hardware from the installed eligible fleet publishers. The personal-use host is limited to its existing collector. |
| Mandatory deployment-tool registration | The [operating contract](../../docs/inventory-reconciliation.md) requires registration and verified retirement. A real staging Linux clone exercised replay, rejection of a stale generation and verified retirement after deletion. Staging also recovered lost replies to registration and retirement for a bound service without repeating either write. The native launchd tool registered a real GUI job, retained it while idle or while its plist remained installed, rejected a stale generation and retired the exact removed job. | Apply the contract in other supported deployment tools. Verify router-side attribution where relevant and provider-specific teardown. Allocator-specific ownership stays in [plan 0001](0001-renewable-test-environments.md). |

The integrated Rust review also verified address rebinding through the
authenticated collector stream and scoped retirement for one real Linux
service. Browser verification covered six configured collectors. Staging records two documented non-host roles
outside guest inventory scope and keeps one offline desktop unconfigured
pending verification. An active bound guest or a configured collector keeps a
machine visible in coverage regardless of its declared scope. The native
systemd deployment sync registered a real discovery service on a scheduled
tick, preserved its record while stopped and retired the record after
confirmed removal. The command also rejected a stale generation. The
unconfigured desktop was unreachable over pinned SSH and lacked a verified
remote power path. Verify remaining supported publishers and deployment tools as they are enrolled.
The [migration release plan](0008-remove-python-and-javascript.md) owns
integrated cutover and rollback. This plan owns wider provider and deployment
coverage after that cutover.

Keep change delivery deterministic. Observations inform discrepancy reports.
Retirement and resource deletion require independent ownership verification
through the authority responsible for the deployment.

[Plan 0006](archive/0006-dashboard-health-review.md) records the review corrections to
manual comparison, catalog freshness and health delivery.
