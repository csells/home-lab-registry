# Central configuration export

`collect.py` is the Backrest export hook. Install alongside `registry/client.py`
and `registry/resolve.py`, preserving their relative layout. The runtime base is
`/opt/backrest`; deployment manifests and credentials remain private.

Sources are resolved from fresh registry presence for each export, with stable SSH
host-key aliases. See [registration](../deploy/metrics/REGISTRATION.md). Scheduling,
encryption, retention and the Google Drive backend remain Backrest/restic/rclone
responsibilities. This module adds no per-machine backup timers or retained copies.

## Guest discovery candidate

The proposed `provider` binding uses a fresh, authenticated Proxmox observation
rather than assigning a hypervisor's identity to its guest. Select `kind:
proxmox-ssh`, enrolled `host_id`, provider SSH `username` and dedicated `key`
filename, immutable guest `vmid`/`uuid`, interface `mac`, and `allowed_networks`.
The source retains its own `username`, `host_key_alias`, optional `port` and archive
`filename`. `hop_username` and `hop_key` opt into a separately restricted tunnel.
These bindings and all key material remain private.

The provider key must execute a forced, read-only command for one authorized guest.
It returns JSON containing `vmid`, `uuid`, `state`, `observed_at` (Unix seconds), and
live QEMU-agent `interfaces`. Its command reads the real guest configuration/state
and addresses, not cached manifest IPs. Host identity/presence is resolved through
the registry before each query. Observations older than 60 seconds or more than two
seconds in the future are rejected (the latter allows small synchronized-clock skew).
The target must be running, match the configured UUID, and have exactly one allowed
IPv4 on the pinned MAC. Missing, stale or ambiguous observations fail closed.

The hop key must permit forwarding only to that qualified guest/port, with no shell.
The export key must allow only the existing export command. Pin host and guest keys
independently through a trusted channel; do not learn a guest key from its discovered
address alone. Existing host-only manifests remain supported. No automatic fallback
to old addresses or unqualified private SSH profiles is provided.

The candidate has real host and one QEMU guest qualification evidence. LXC, HAOS,
remaining fleet sources and full application startup recovery remain unqualified.
This does not authorize production installation or promotion.

## Staging review

`review/server.py --report /path/to/redacted-report.json --port 9084` serves a
loopback-only results page and evidence JSON. Supply only redacted source labels,
verification statements and remaining gaps. It serves exactly `/`, `/report.json`
and `/healthz`, never filesystem paths, credentials or backup archives. A separately
owned authenticated relay may expose it for review; the page does not run tests or
promote a build when refreshed.
