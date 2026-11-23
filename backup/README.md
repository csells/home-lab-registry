# Central configuration export

`lab-backup-collect` is the Backrest export hook. Build it using `cargo build --locked --release --manifest-path backup/rust/Cargo.toml`. The runtime base is
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
For LXC use `guest_kind: lxc` and the exact root-volume `volume_id` instead of
inventing a UUID. Require its independently configured interface MAC as well.
The source retains its own `username`, `host_key_alias`, optional `port` and archive
`filename`. Optional `export_key` selects a separately restricted exporter key;
the default remains `collector-key`. `hop_username` and `hop_key` opt into a
separately restricted tunnel.
These bindings and all key material remain private.

The provider key must execute a forced, read-only command for authorized guests.
`lab-backup-proxmox-observe --policy /path/to/private-policy.json` accepts only
`discover ID` where the ID is in the operator-owned policy. That policy contains
`node` and a `guests` object mapping decimal ID strings to `qemu` or `lxc`.
It returns `vmid`, `kind`, `uuid` or `volume_id`, `state`, `observed_at` (Unix
seconds), and live `interfaces`. QEMU uses its guest agent; LXC uses the real
container's network namespace through `pct exec`. It reads real configuration/state
and addresses, not cached manifest IPs. Host identity/presence is resolved through
the registry before each query. Observations older than 60 seconds or more than two
seconds in the future are rejected (the latter allows small synchronized-clock skew).
The target must be running, match the configured UUID or volume, and have exactly one allowed
IPv4 on the pinned MAC. Missing, stale or ambiguous observations fail closed.

The hop key must permit forwarding only to that qualified guest/port, with no shell.
The export key must allow only the existing export command. Pin host and guest keys
independently through a trusted channel; do not learn a guest key from its discovered
address alone. Existing host-only manifests remain supported. No automatic fallback
to old addresses or unqualified private SSH profiles is provided.

For a non-root observer account, authorize only the exact root-owned observer and
policy command through sudo. Preserve only `SSH_ORIGINAL_COMMAND` for that account
using a scoped sudoers `env_keep` entry, since the observer validates it. Do not
grant general environment, shell, arbitrary guest-exec or provider mutation access.
The policy, observer and parent directories must not be writable by that user.

Actual fleet acceptance includes host, QEMU, LXC, HAOS and native Mac exports.
Collection/restore evidence does not prove every application's external integrations
or clean source rebuild. Current results and permissions belong in the authenticated
registry, not public source. Production promotion still requires explicit approval.

Archive validation checks gzip completion and tar headers without rewriting the
export. Mac PAX extension metadata remains in the original bytes for native restore;
it does not produce GNU tar's repeated unknown-extension warnings on Linux.

Run unit checks with `cargo test --locked --manifest-path backup/rust/Cargo.toml`.
Real admission, credential and recovery acceptance must additionally run against
the actual lab from disposable staging.

## Staging review

For a controller recovery drill, restore the required Backrest bootstrap files
from an explicit encrypted snapshot into a fresh private directory. Use the
list in `rust/src/bin/lab-backup-verify.rs` to select the files and `restic restore --verify`
to verify their bytes. Run `lab-backup-verify RESTORE_ROOT`
before starting the recovered controller. The check reports counts without
printing restored credentials or configuration values. Keep the return of
the Backrest service and repository access as separate runtime checks.

`lab-backup-review --report /path/to/redacted-report.json --port 9084` serves a
loopback-only results page and evidence JSON. Supply only redacted source labels,
verification statements and remaining gaps. It serves exactly `/`, `/report.json`
`/healthz` and the generated `/checks.js`. It omits filesystem paths, credentials or backup archives. A separately
owned authenticated relay may expose it for review; the page does not run tests or
promote a build when refreshed.

The review server can optionally take `--check /path/to/operator-owned-checks`
and `--origin https://authenticated-review.example`. Its explicit POST action runs
only that fixed command, admits one job at a time, enforces the configured browser
origin, and returns bounded status without command output. For a readable,
non-executable qualification script, pass an explicit interpreter with
`--check-interpreter /absolute/path/to/interpreter`. Refresh remains read-only.
Use a dedicated disposable staging runtime and the minimum permissions needed by
those checks; qualification can collect real exports and write temporary staging.

Run `npm ci --prefix dashboard/frontend` and `npm run build:backup-review --prefix
dashboard/frontend` with the dashboard's pinned TypeScript toolchain. Install
`backup/rust/target/release/backup-review.js` beside `lab-backup-review` or pass its
path to `--script`. The review service requires the browser bundle at startup.
