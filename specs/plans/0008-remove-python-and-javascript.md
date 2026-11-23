# Finish the Rust and TypeScript migration

Status — active. Use commit `0230fe1` as the executable baseline from
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
