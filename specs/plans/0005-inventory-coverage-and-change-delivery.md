# Inventory coverage and change delivery

Status: active. Production has streamed guest inventory and manual reconciliation.
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
| Registry metadata change delivery | Stable reads at startup and on **Reconcile now**. Staging resumed from a known stream cursor after two disconnected writes and retried an HTTP 503 from the first refresh. The current catalog appeared in 3.55 seconds with all six configured collectors verified. The review app also completed manual reconciliation and opened Leases at mobile width. | Verify through the owner's public review session. Production rollout remains a separate step. |
| OS-native lifecycle events | Collectors sample local Proxmox and Tart inventory and stream changes. A disposable Tart check observed create, start, stop and delete before the sampling fallback. | Qualify native delivery and lost-event recovery by provider. Verify the deployed path and keep sampling or unsupported providers visible in coverage. |
| Native/cloud presence discovery | Stable verifies guest bindings from authenticated hypervisor observations. Staging verified a real systemd unit and a cloud endpoint using scoped publishers. It rejected a report for an old binding and surfaced explicit absence. | Roll out and verify supported publishers with real deployment identities. Exercise stale and unavailable sources separately from absence. |
| Physical hardware refresh | The candidate adds OS, architecture, logical CPU count and RAM capacity with their own observation timestamp over the authenticated guest stream. A temporary collector on a real Proxmox host matched local CPU and RAM readings, and the Mac passed the native sampler. The dashboard distinguishes expired and unverified facts. | Record fleet rollout coverage and decide which further hardware facts warrant dynamic discovery. |
| Mandatory deployment-tool registration | The [operating contract](../../docs/inventory-reconciliation.md) requires registration and verified retirement. A real staging Linux clone exercised replay, rejection of a stale generation and verified retirement after deletion. Staging also recovered lost replies to registration and retirement for a bound service without repeating either write. | Apply the contract in supported deployment tools. Verify router-side attribution where relevant and provider-specific teardown. Allocator-specific promotion and ownership stay in [plan 0001](0001-renewable-test-environments.md). |

Keep change delivery deterministic. Observations inform discrepancy reports.
Retirement and resource deletion require independent ownership verification
through the authority responsible for the deployment.
