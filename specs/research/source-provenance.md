# Sources, evidence and consolidation

Consolidated 2026-11-20 into the public Home Lab Registry project at the owner's
request. This transfers reusable planning/research ownership, not runtime services
or deployment data. Earlier private planning copies are retired in favor of these
canonical documents; original owner-controlled evidence remains in private history.

## Maintained sources

| Source | Treatment |
|---|---|
| Existing [vision](../vision/home-lab.md) | Expanded with heterogeneous allocation, first-class Apple pooling and explicit public/private source ownership |
| October 19 [renewable-environment plan](../plans/0001-renewable-test-environments.md) | Remains the canonical allocation plan; preserves its contract, autonomous renewal, operation fencing, checkpoint/recovery, safety and Linux-first implementation slice; adds Apple delivery and placement findings |
| [Reconciliation candidate plan](https://github.com/csells/home-lab-registry/blob/277fbda89da4dcd1002d37665e3477ae93ad937e/specs/plans/0003-inventory-reconciliation.md) | Separate review work preserved; identity/freshness/event requirements integrated without claiming fleet rollout or promotion |
| [Current architecture](../architecture/current.md) | Dated implemented baseline; verify source revisions and live state before relying on availability |
| [Usage audit](2026-11-20-bb-home-lab-usage-audit.md) | Redacted synthesis of 28 retrieved conversation groups across 20 threads; coverage and uncertainty are recorded in the audit |
| [Platform-source plan](../plans/0004-platform-source-consolidation.md) | Separates reusable source from private deployment glue and records component-specific source parity |
| Private operational/security material | Used to identify staging, recovery and identity boundaries; vulnerability details, credentials, inventory and superseded instructions are excluded; current findings remain in private operational records |

The public research is a synthesis, not a raw transcript archive. Internal thread
locators, personal source paths, host identities and runtime catalog exports were
removed for publication. Project documentation/source links are retained in the
audit. Original private evidence is accessible to its owner, not to public readers;
historical reports must be requalified against actual interfaces before implementation.

## Decision and authority

The evidence favors one allocation/lifecycle authority over existing Proxmox and
Apple backends, while testing Cloudpods against the same contract. It does not
establish that a cluster or platform rebuild is necessary, or that a custom controller
is automatically simpler. Component licensing and exact runtime behavior need fresh
verification when choosing a build.

The expanded charter includes reusable allocation, provider adapters and Mac-worker
tooling. Deployment-specific configuration remains operator-owned; application
repositories retain their source. Documentation migration does not authorize a
hypervisor migration, service cutover, promotion, credential copy or destructive cleanup.
Plans remain active until their required work is verified, rather than being marked
complete merely because requirements were consolidated.
