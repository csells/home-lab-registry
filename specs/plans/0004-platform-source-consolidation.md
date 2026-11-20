# Reusable platform source and deployment boundaries

Status: proposed follow-up, 2026-11-20. Documentation migration only; no service source,
deployment definition or runtime has been imported, changed or promoted by this plan.

## Outcome

Reusable home-lab infrastructure has maintained, reviewable source in this public
project. Operator-specific deployment glue has an explicit private source owner.
Neither public nor private Git stores credentials, live inventory or recovery archives.

## Source ownership

| Component | Source home and work required |
|---|---|
| Registry, dashboard, collector, browser terminal and reconciliation | Existing source in this repository; preserve current review branches and source ownership |
| Environment allocation and lifecycle | Reusable controller and provider adapters here, following [plan 0001](0001-renewable-test-environments.md); one reservation authority and a separate provider-mutation authorization boundary |
| Mac worker and service-management tooling | Reusable executor, reviewed setup/helper templates, stable automation identity procedures and image tooling here; audit existing helper source before import; signing identities, certificates, accounts and device assignments stay private |
| Central backup integration | Generic `backup/collect.py` already lives here; compare other deployed exporters/controller scripts with tracked source before extracting reusable hooks; operator schedules, deployment manifests, credentials and recovery inputs remain private |
| Plex sync/maintenance, Alook deployment and mail dynamic DNS | Audit custom integration source and maintain deployment-specific units/templates privately; these use third-party products, not new internally authored Plex/Alook/DNS services |
| Device firmware and household configuration | Establish actual custom content and ownership first; publish reusable tooling only after review; household configuration remains private |
| Applications and shared services with existing repositories | Preserve their existing source owners; correct stale private registry pointers instead of importing duplicate code |

The original assessment found candidate gaps in maintained source ownership, not
proof that code had never appeared on another branch or account. A missing registered
repository pointer is unknown ownership, not permission to relocate code.

## Work

- [ ] Resolve each candidate's actual deployed revision, Git origin/history and owner
  through the authenticated registry and source checkout. Compare deployed files
  with source; keep private findings in operational records.
- [ ] Separate reusable code/templates from deployment values and incident scripts.
  Review for credentials, private catalog data, personal data, licensing and attribution.
- [ ] Import one bounded reusable component at a time, with placeholders/secret
  references, source history where useful, and build/deployment/recovery instructions.
- [ ] Give deployment-specific assets a maintained private source home. Configuration
  backups provide recovery evidence, not a substitute for reviewed source ownership.
- [ ] Verify source/deployment parity and backup coverage before updating registry
  source pointers; source relocation alone does not authorize deploying a changed build.
- [ ] Verify service code changes in a disposable VM against actual lab interfaces,
  preserve stable state, and obtain explicit staged-candidate promotion approval.

## Completion

A fresh agent can locate the correct source revision and reproduce a bounded
component without mining transcripts or committing credentials. Published source
contains only reviewed reusable content. Actual deployment state and recovery
evidence remain in the registry and configured encrypted backup service.
