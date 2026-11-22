# Reusable platform source and deployment boundaries

Status: in progress, 2026-11-20. Reviewed operator-specific integration source has
been captured in the operator's private deployment repository. No public service
source import, runtime change or promotion is claimed by this documentation update.

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

## Verified private-source progress (2026-11-20)

The operator's private workspace now maintains reviewed Plex sync code/units, Alook
service/startup definitions, mail-ddclient unit and selected backup exporters/playbooks.
An initial 19 files were source/hash checked; seven additional integration definitions
or address/identity templates followed the dedicated-exporter review. Secrets, runtime
inventory and archives were excluded. Registry source pointers identify the private
source revision. This completes the bounded private source-home slice, not every
reusable component import or service recovery requirement above.

The backup collector, scoped source resolution and portable archive validation
were merged in [PR #1](https://github.com/csells/home-lab-registry/pull/1).
The new collector is running and the dedicated staging resources were retired.
The fleet qualification verified encrypted round trips for 17 remote collection
paths. Private SSH profiles preserve required tunnels and host-key aliases.
The deployment record owns current source identities, coverage and recovery
evidence. Disabled historical plans retain their archives.

Remaining source work covers the reusable Apple worker and image/setup tooling,
device tooling with verified ownership, and any exporters outside the reviewed
collector delivery. Check the actual source revision, license and deployment
parity before importing each component. The private operations plan tracks
individual application and hardware recovery limits.

## Completion criteria

A fresh agent can locate the correct source revision and reproduce a bounded
component without mining transcripts or committing credentials. Published source
contains only reviewed reusable content. Actual deployment state and recovery
evidence remain in the registry and configured encrypted backup service.
