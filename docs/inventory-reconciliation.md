# Keeping inventory accurate

The dashboard compares registered deployments with complete, authenticated guest
reports from enrolled metrics collectors. Each host has one persistent inventory
subscription: a cached initial report, content-change events, and 60-second freshness
heartbeats. No scheduled dashboard guest-list GETs occur. Resource streaming remains
demand-driven. Browser inventory updates use Server-Sent Events.

Collectors sample their local hypervisor at the configured interval, normally
30 seconds. Enrolled native event sources can also wake a collector. An idle
subscriber triggers no additional hypervisor reads. **Reconcile now** reloads
registry metadata and reconnects to the collectors' current cached reports.
The dashboard subscribes to authenticated registry catalog changes. The dashboard
reports catalog feed loss independently from collector coverage and resumes after
the connection recovers. **Refresh** requests an authoritative catalog read.
**Reconcile now** returns a comparison ID. `GET /api/reconciliation` exposes the
current comparison ID, state and report under dashboard authentication.

## Register deployments

Every deployment must have an owner, a purpose, and a structured runtime binding in
its service record. For a Proxmox or Tart guest:

```json
{"resources":{"runtime_binding":{"kind":"guest","machine_id":"machine-worker","guest_id":"123"}}}
```

For native or cloud applications, use `kind: "native"` or `kind: "external"`, and
record the machine or provider and service identity. These declarations identify
placement; they do not prove the application exists. Application health and backup
checks remain separate. Shared guests can contain several service records. Existing
structured `guest_id` / `vmid` fields remain supported; prose deployment descriptions
are never parsed as identities.

Register disposable workloads with `resources.expires_at` (ISO timestamp including
timezone). Renew as work continues; expired leases are warnings, never permission to
stop someone else's work. Templates belong in the parent machine's structured template
list and do not require a service record.

Before reporting a deployment complete, register it, read it back, and reconcile.
Resolve unregistered/missing guest findings and incomplete placement metadata, or
record a concrete reason and owner. An unavailable collector is incomplete verification,
not evidence of deletion. A stopped guest still exists and consumes storage.

## Retire deployments

A native systemd deployment tool can call
`lab-register-deployment --config PATH sync-systemd RECORD MANIFEST` on the
deployment host after installation and again after removing the unit and
reloading systemd. Use the original active manifest, including its exact unit,
owner, and deployment generation. A stopped unit still has a deployment and
stays registered. An inconclusive lookup leaves the catalog alone. Keep catalog
writer credentials in the deployment tool's private operator context.

After authorized teardown, verify that the exact guest and its dedicated storage are
gone, preserve required external evidence, retire the corresponding records, then
reconcile again. Never retire records merely because a host failed to report.

## Interpret the report

- **Unregistered guest:** fresh discovery has no service or machine binding.
- **Missing guest:** a registered binding is absent from a fresh complete host report.
- **Collector unverified:** absent, expired, invalid or failed observations.
- **Collector unconfigured:** an active machine has no configured guest
  collector and its guest-inventory scope is still unknown.
- **Guest inventory outside scope:** a machine with a verified non-host role
  can declare `resources.guest_inventory.not_applicable_reason`. The dashboard
  ignores this declaration when a collector is configured, an observation
  exists, or an active guest record names the machine as its parent.
- **Deployment unmapped:** service presence cannot be joined to structured placement.
- **Expired lease:** ownership needs review or renewal; no automatic deletion.
- **Duplicate machine:** two machine records claim the same guest. Several services
  sharing a guest are valid.

Observed guest lists and their timestamps are the current runtime view. Historical
`guest_snapshot` and task notes are evidence only, never runtime authority. Reconciliation
is read-only: it does not overwrite registry intent or erase historical observations.
Native process discovery, cloud inventory and hardware changes are outside guest-report
coverage and must not be advertised as verified by this report.

## Stream rollout

Collectors need `/guests/stream`. Unsupported or unauthorized endpoints are reported
unverified and wait for explicit reconnection or configuration changes. There is no
snapshot-polling fallback. Broken supported streams reconnect with bounded backoff;
stale observations remain explicitly unverified. A stream event cannot authorize
workload deletion.

The staged agent reads a real hypervisor through its own read-only audit token. The
private endpoint override changes guest discovery only; it does not misrepresent the
staging VM's CPU/RAM as the physical host's metrics. Production verification covered all six configured collectors on 2026-11-20.
