# Keeping inventory accurate

The dashboard compares registered deployments with complete, authenticated guest
reports from enrolled metrics collectors. Each host has one persistent inventory
subscription: a cached initial report, content-change events, and 60-second freshness
heartbeats. No scheduled dashboard guest-list GETs occur. Resource streaming remains
demand-driven. Browser inventory updates use Server-Sent Events.

The collector's existing local hypervisor sampling remains at its configured interval
(default 30 seconds); this change removes central fleet polling, not that local sampling.
An idle subscriber triggers no additional hypervisor reads. OS-native lifecycle event
collection is not implemented. **Reconcile now** reloads registry metadata and reconnects
to the collectors' current caches; it does not force a hypervisor rescan or launch an agent.
Registry metadata is loaded at startup and on this explicit action, not automatically
pushed by the current registry API. A registry metadata change feed remains follow-up
work; do not confuse live guest events with live edits to service records.
`GET /api/reconciliation` exposes the same report under dashboard authentication.

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

After authorized teardown, verify that the exact guest and its dedicated storage are
gone, preserve required external evidence, retire the corresponding records, then
reconcile again. Never retire records merely because a host failed to report.

## Interpret the report

- **Unregistered guest:** fresh discovery has no service or machine binding.
- **Missing guest:** a registered binding is absent from a fresh complete host report.
- **Collector unverified:** absent, expired, invalid or failed observations.
- **Deployment unmapped:** service presence cannot be joined to structured placement.
- **Expired lease:** ownership needs review or renewal; no automatic deletion.
- **Duplicate machine:** two machine records claim the same guest. Several services
  sharing a guest are valid.

Observed guest lists and their timestamps are the current runtime view. Historical
`guest_snapshot` and task notes are evidence only, never runtime authority. The candidate
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
staging VM's CPU/RAM as the physical host's metrics. Fleet rollout awaits promotion.
