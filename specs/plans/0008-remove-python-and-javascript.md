# Finish the Rust and TypeScript migration

Status — implemented and qualified in disposable staging. Production promotion
is a separate operator decision. Use commit `0230fe1` as the executable baseline from
[Plan 0007](0007-react-typescript-and-rust-port.md). Keep its history accessible
for contract comparison and rollback. Retire replaced files through ordinary
commits after equivalent behavior is verified.

| Slice | Acceptance |
| --- | --- |
| Dashboard Resources and browser checks | TypeScript owns graph construction, rendering and browser tests. Preserve ELK worker isolation, containment, dependency routing, filters and failure handling. Generate the pinned third-party worker during the build. |
| Registry, dashboard and operators | Use Rust for service entry points, operator and task CLIs, deployment registration, network audit, and portable contract tests. Preserve scopes, transport, credentials, receipt semantics and durable state. |
| Allocator and workspace lifecycle | Use Rust throughout Proxmox and Tart operation dispatch, export, checkpoint transport, task launch, recovery, alerts and scoped storage. Preserve provider identity, failure outcomes, admission, access and safe cleanup. |
| Backup and recovery | Preserve validation, capture, encrypted storage, restore and operator checks with Rust executables. Verify recovery through the real staging storage and stopped guest boundaries. |
| Full staged candidate | Build, run and test from a clean checkout with Rust and TypeScript tooling. Remove the first-party Python and JavaScript source, legacy entry points and language-specific CI jobs. Preserve the existing Go collector and verify its contracts. Exercise the full disposable candidate through real interfaces and leave production promotion for separate approval. |

Browser bundles and the pinned ELK worker are generated build artifacts.
Preserve the deployed HTTP contracts, operator options and user-visible
workflows. For each slice, run behavior tests against the baseline, run the
ported tests against the new implementation, and record real staging evidence
where the behavior needs it.

## Qualification

- The branch removes 95 tracked Python and JavaScript files and retains
  `0230fe1` in its ancestry. The existing Go collector is unchanged.
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
release. The on-demand slot's capacity credential returned HTTP 401 during
qualification.

The macOS client links system libraries. The Windows x86-64 task and registry
clients cross-built successfully. The native Windows workflow covers TLS,
private configuration ACLs and executable packaging when run on its Windows
runner. Production promotion remains a separate operator decision.
