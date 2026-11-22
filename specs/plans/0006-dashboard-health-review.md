# Dashboard health and reconciliation corrections

Status — staged. The review found eight defects in the dashboard, health
publisher, and collector. Runtime revision `d4f9607` contains the fixes.
Production promotion remains pending.

| Requirement | Regression evidence to capture |
| --- | --- |
| Correlate manual comparison with its accepted request | A delayed comparison can coexist with unrelated updates. Only its matching completed result settles the UI. The authenticated status path also works with the browser stream interrupted. |
| Show catalog freshness independently of collector freshness | A lost or rejected catalog feed marks cached registry information unverified while guest reports continue. Reconnection and Refresh restore verified metadata through the real registry. |
| Share the HTTPS-only registry read transport | Dashboard refresh and terminal launch use the hardened transport, which rejects redirects and incomplete reads. |
| Propagate DHCP endpoint changes | A committed change to an enrolled address wakes cached catalog clients. Renewals at the same address avoid unnecessary refreshes. |
| Align guest identity and lifecycle | Structured and legacy bindings behave consistently for comparison and runtime. A planned record neither reports a missing guest nor conceals an unregistered guest. |
| Keep lease problems current | An already open dashboard reflects missed deadlines, export failures, and recovery. Read failures show incomplete coverage and recover without requiring unrelated guest changes. |
| Bound work triggered by health renewals | A health-only update refreshes the affected observation through the change feed. Routine observations leave guest subscriptions and lease ownership untouched. |
| Compare configured HTTP status correctly | Probe status comparison handles matching non-2xx responses and preserves the numeric status for diagnosis. Redirect destinations remain outside the probe. |

## Staging acceptance

Exercise the authenticated dashboard, health publisher, registry, all configured
collectors, and lease review in the full disposable candidate. Record the source
revision and results. Compare the preview catalog against live active and retired
records so old review metadata cannot produce false findings.

## Verification

- Red-green regression tests exercised all eight findings. The dashboard,
  registry, allocator, backup and address-audit suites passed 132 tests.
  Collector race tests, the resource graph, and browser comparison fixtures also
  passed.
- The complete disposable candidate used the normal registry, collector, and
  allocator interfaces. A timestamped copy of live catalog records included
  retirements, and all six configured collectors reported. No retired record
  was listed as missing.
- Mobile and desktop browser verification confirmed health-only updates,
  catalog and allocator outage recovery, the Leases page, and authoritative
  Refresh and request-specific Reconcile with browser SSE blocked.
- A real lease crossed its deadline, then a missing workspace caused an export
  failure that held the disk. Both findings appeared in the preview. After the
  workspace was initialized, the retry cleared the finding, released the slot,
  and produced a downloadable checkpoint that matched its receipt. The
  [Linux recipe follow-up](0001-renewable-test-environments.md#initialize-the-managed-linux-workspace-before-declaring-readiness)
  tracks workspace preparation at admission.
- Transport enforcement and DHCP-change notifications passed focused regression
  tests. The lab's production addresses stayed in place.

The allocator currently exposes an authenticated issue summary. The dashboard
shares one viewer-driven read, normally every twenty seconds, and uses its
existing event stream to deliver changes. A reported deadline can trigger an
earlier read. The read pauses once the last viewer disconnects.
