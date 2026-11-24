# Finish the Rust and TypeScript migration

Status: active, reconciled 2026-11-23. The registry, dashboard, five
eligible collectors and 18 Backrest export plans now run the ported components
in production. The operator accepted a targeted staging exception for this
rollout. The allocator and associated provider and capture authority remain
in staging.

| Slice | Acceptance |
| --- | --- |
| Dashboard Resources and browser checks | TypeScript owns graph construction, rendering and browser tests. Preserve ELK worker isolation, containment, dependency routing, filters and failure handling. Generate the pinned third-party worker during the build. |
| Registry, dashboard and operators | Use Rust for service entry points, operator and task CLIs, deployment registration, network audit, and portable contract tests. Preserve scopes, transport, credentials, receipt semantics and durable state. |
| Allocator and workspace lifecycle | Use Rust throughout Proxmox and Tart operation dispatch, export, checkpoint transport, task launch, recovery, alerts and scoped storage. Preserve provider identity, failure outcomes, admission, access and safe cleanup. |
| Backup and recovery | Preserve validation, capture, encrypted storage, restore and operator checks with Rust executables. Verify recovery through the real staging storage and stopped guest boundaries. |
| Full staged candidate | Build, run and test from a clean checkout with Rust and TypeScript tooling. Remove the first-party Python and JavaScript source, legacy entry points and language-specific CI jobs. Preserve the existing Go collector and verify its contracts. Exercise the full disposable candidate through real interfaces. |

Browser bundles and the pinned ELK worker are generated build artifacts.
Preserve the deployed HTTP contracts, operator options and user-visible
workflows. For each slice, run behavior tests against the baseline, run the
ported tests against the new implementation, and record real staging evidence
where the behavior needs it.

## Qualification

- The branch removes 95 tracked Python and JavaScript files and retains
  `0230fe1` in its ancestry. The migration retains the Go collector. The integrated
  branch also carries the collector upgrades from [plan 0005](0005-inventory-coverage-and-change-delivery.md).
- A clean Git archive built the Rust executables, React bundle and backup
  review script on Debian. TypeScript checks, Rust contract tests, browser
  suites and Go race tests passed. The dedicated Rust lease-review test also
  matched frozen legacy responses against the same real staging SQLite snapshot.
- The authenticated dashboard opened every tab and verified six live
  collectors. A real probe failure and recovery updated Discrepancies.
  Manual reconciliation finished while the browser stream was interrupted,
  and the stream recovered independently afterward.
- Scoped Linux and Apple leases exercised acquisition, restart recovery,
  checkpoint export, encrypted readback and matching-environment restore.
  A Linux replacement exported its restored workspace and its files remained
  readable after release. The native CUDA probe verified device memory in an
  unprivileged disposable guest, then verified its removal and capacity release.
- A real systemd collector announced its address without a login. Its mTLS
  stream and pinned SSH export followed an isolated interface address change.
  The test verified scoped runtime publishing and exact retirement. Its
  passing 60-second idle sample measured 0.017% CPU and 8.63 MiB RSS (0.587%
  of guest RAM). An earlier sample exceeded the memory threshold on the same
  VM. The passing sample records a measurement rather than a fleet guarantee.
- Rust backup review verified a controller bootstrap restored by encrypted
  Restic. The scoped storage command verified write, byte-for-byte readback,
  exact expiry and preservation of a snapshot owned by another workload.
  This exercised the scope command in a disposable repository. The staged
  lease workers separately exercised the enrolled encrypted storage path.
  The Proxmox observer read the approved QEMU guest address and rejected an
  unlisted guest ID.
- The address audit read live router reservations, the registry and a
  Linux guest through pinned SSH. The macOS collector read local interface
  evidence. KVM/AMT authentication and offline disk inspection were not
  repeated fleet-wide as part of this source migration.

The staged runtime retains its existing catalog, ledger witness, scoped
credentials, and rollback artifacts. Stable deployments and unrelated lab
services were not changed.

## Regression qualification

The staged runtime at `d90e512` includes the parity corrections against main and
the pre-migration allocator. The catalog and checkpoint services enforce read
and write deadlines. The dashboard refreshes lease findings on demand, reports
incomplete audits, reloads authorized origins, and isolates failing collector
endpoints. The audit can send its native reader temporarily over pinned SSH
and to running Proxmox guests through their existing guest interface.

The authenticated preview verified manual refresh, lease history, live charts,
and six of six configured collectors. The native audit queried real router
reservations and read a disposable guest through both SSH and the QEMU guest
agent. The backup review ran its explicit check against an encrypted Restic
restore. A fixed-slot Linux lease uploaded a checkpoint, released, and returned
its retained evidence. A matching replacement preserved its file mode and
fractional PAX timestamp, and its final checkpoint remained readable after
release. This established legacy archive read compatibility. A separate
differential test found that the native writer discarded fractional timestamps.

After renewal of expired on-demand Proxmox and stopped-disk capture grants,
a live task cloned and booted an on-demand guest, verified SSH and the requested
resource sizes, released it and recovered its checkpoint byte-for-byte. The
guest, disk and temporary task credentials were verified removed. The frozen
ledger comparison also passed for current, issues, released and all views.

The timestamp fix writes an exact PAX modification time in
[the workspace writer](../../allocator/rust/workspace/src/unix.rs). The
[file and directory regression](../../allocator/rust/tests/workspace_contract.rs)
failed before the patch and passed afterward. Both writers and both readers
preserved the timestamp in all four pairings. The separate disposable Linux
build exercised the real archive and restore tools. All 49 enabled allocator
tests passed. The rebuilt Tart worker preserved content, mode and the exact
timestamp in a recovered real lease archive.

The integrated preview passed Rust service checks, TypeScript and browser
suites, and the Go collector race tests. The browser run with `12caa8f`
verified the deployed bundle, live charts, lease history, refresh and manual
reconciliation. In the Linux drill, the stopped-disk capture truncated
fractional timestamps. With the PAX capture in `66f242f`, a real Debian lease
exported and restored exact content, permissions and timestamps. Its replacement
then exported the same file metadata on release.

A Tart clone remained running without an SSH address and release held it in
Starting. The enrolled worker stopped the owned clone and confirmed its state.
Revision `66f242f` allows the owner to request export after an exact, completed
start and verified stop. Revision `0bb40d3` leaves a Proxmox disk with changed
start history held while allowing a different eligible lease to continue. The
staged exporter selected the Tart lease, read back its encrypted checkpoint and
released the owned workspace.

The matching Tart restore exposed two separate failures. Staging used Tart
2.40.1 to recover from control-socket accept errors. The real VirtioFS share
returned Darwin `ENOTSUP` for an exclusive rename and supported hard-link
creation. The restore helper now uses that fallback without replacing an
existing directory entry. A matching Tart VM restored the exact bytes, mode
and timestamp and retained them when the replacement released and re-exported.

The macOS client links system libraries. Windows x86-64 task and registry
clients cross-built. Native Windows TLS, private configuration ACLs and
packaging remain deferred. This concerns where the client runs. Windows guest
provisioning is a separate capability in [plan 0001](0001-renewable-test-environments.md).

## Held export and address-audit follow-up

A staging operator captured and verified the held Proxmox workspace after
a repair start. The allocator rejected a different task and a concurrent
operator attempt. The offline reconciliation operation recorded the exact
successful start and preserved the storage identity. The normal final export
then completed, Proxmox verified the guest and physical disk were absent,
and an independent read recovered the empty encrypted checkpoint.

The native address audit verified saved Proxmox DHCP and cloud-init profiles
without booting the guest. Live reads from four external KVMs and two Intel
AMT interfaces matched the router's current evidence. Both tested stopped
Tart templates yielded DHCP settings and detached cleanly. Unavailable
inventory remained unverified. A separate read-only device check matched the
ESPHome appliance's reported build with its recorded factory source and live
router entry. Its DHCP assignment is inferred from the matched source and
provisioning history. The native audit shows the appliance as unverified when
it has not refreshed that evidence.

## Production rollout and remaining qualification

The production registry retained the catalog and existing TLS and authentication
configuration. The dashboard kept its public access gate and verified all six
guest streams, a matching manual reconciliation receipt and an authenticated
inventory event. The update verified five new native collector builds. The Rust
Backrest hook generated fresh encrypted registry and host backups. The restored
registry archive passed SQLite integrity. The registry records pre-change and
post-change snapshots, exact artifact digests and available rollback copies.

The earlier registry and dashboard rollout used an explicit staging exception.
The subsequent combined production release used the checksummed bundle tested
in staging, the signed ledger handoff and production provider, capture and
recovery scopes. Keep Windows client TLS, private configuration ACLs and
packaging deferred.

## Carried from archived plans

- [Plan 0006 production verification](archive/0006-dashboard-health-review.md#production-verification)
  carries all eight dashboard corrections forward, including authenticated
  session recovery, live discrepancy updates and lease export-failure recovery.
- [Plan 0007 acceptance and delivery rules](archive/0007-react-typescript-and-rust-port.md#order-of-delivery)
  carry forward production contract parity, measured stream and recovery
  behavior, and separate rollback artifacts for the integrated deployment.
