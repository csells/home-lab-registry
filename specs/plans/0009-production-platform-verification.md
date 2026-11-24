# Production platform verification

Status: active, reconciled 2026-11-24. The combined registry release runs in
production and its dedicated staging environment has been retired. The live
registry records the exact release, rollback and encrypted restore evidence.

## Carried from archived plans

- [Dashboard health review production verification](archive/0006-dashboard-health-review.md#production-verification)
  requires exercising session recovery, live discrepancy updates and lease
  export-failure recovery with the approved build. Production authenticated
  refresh, the matching Reconcile now receipt, all configured collector
  streams and normal checkpoint recovery passed. Verify authenticated session
  reconnection and live discrepancy appearance and recovery on the production
  dashboard. Exercise a benign registry change after reconnecting, as tracked
  under [inventory change delivery](0005-inventory-coverage-and-change-delivery.md).
  A controlled production export failure held the lease, delivered an email
  the operator confirmed receiving, and returned the slot after verified
  checkpoint recovery. Verify that the dashboard shows the hold and clears
  it after recovery.
- [React, TypeScript and Rust delivery rules](archive/0007-react-typescript-and-rust-port.md#delivery-rules)
  call for measuring idle work, stream behavior and recovery against the
  prior candidate. Keep the staged baseline and qualify a comparable
  full-platform measurement. Plan 0001 owns workload checkpoint and
  replacement timing. Plan 0005 owns publisher and event delivery coverage.
- [Rust and TypeScript migration](archive/0008-remove-python-and-javascript.md#production-rollout-and-remaining-qualification)
  verified the combined cutover and normal production workflows. The
  production dashboard, registry, allocator and local lease workers share
  the same release, and the encrypted backup restored matching catalog and
  allocator state. Retain the Windows validation below.

## Deferred verification

- The operator deferred the independent Google Drive reauthorization drill.
  Verify recovery with a fresh read-only grant on a clean backup controller
  when the drill resumes. The existing controller restored the production
  snapshot through its enrolled connection.
- The operator deferred signed-in Cloudflare Access revocation. Exercise the
  effect on an approved signed-in identity and measure the revocation delay
  when the drill resumes. Routine dashboard session recovery is tracked above.
- A real Windows host still needs to install the packaged task and registry
  clients and exercise TLS and credential ACLs against lab interfaces. Native
  CI passed the transport and ACL tests and generated the client bundles.
  Windows guest provisioning remains separately deferred in
  [plan 0001](0001-renewable-test-environments.md#remaining-allocator-acceptance).
