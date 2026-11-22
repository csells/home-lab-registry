# Dashboard health and reconciliation corrections

Status — active. Review of the staged dashboard, health publisher, and collector
found eight defects. Delivery requires the full candidate in disposable staging,
with live dependencies and separate approval for promotion.

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
