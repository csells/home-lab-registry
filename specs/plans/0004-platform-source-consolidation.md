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

Controller/public collector drift remains open. The old manifest uses static SSH
destinations; the public collector requires enrolled target identities and current
SSH presence. Guest export targets must not be replaced with physical-hypervisor IDs.
Real-registry resolver preflight exposed identity/freshness gaps. Dragon management
was repaired and actual-source collector staging passed restore/integrity, repeat
and failure checks with authorized reader-only access and a dedicated export key.
Fleet-wide identity/transport qualification, complete recovery and promotion remain
open; the stable collector is unchanged. Several private SSH profiles require
explicit tunnel and host-key-alias preservation; direct endpoint substitution alone
is insufficient. Disabled historical plans retain their archives and are distinct
from active backup sources.
Track current blockers in the central-backup registry record and private deployment
plan rather than duplicating live fleet inventory here.

## Completion criteria

A fresh agent can locate the correct source revision and reproduce a bounded
component without mining transcripts or committing credentials. Published source
contains only reviewed reusable content. Actual deployment state and recovery
evidence remain in the registry and configured encrypted backup service.
