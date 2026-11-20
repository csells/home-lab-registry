# Working on Home Lab Registry

This public repository owns the reusable home-lab platform: registry/dashboard,
collectors and reconciliation, environment allocation/lifecycle, provider adapters,
and shared Apple-worker tooling. Preserve application source ownership and private
deployment configuration. Proposed contracts are not available APIs.

## Read by task

- For product, placement or platform decisions, read `specs/vision/home-lab.md`.
- For environment allocation, leases, recovery or Mac workers, read
  `specs/plans/0001-renewable-test-environments.md`.
- For source imports, read `specs/plans/0004-platform-source-consolidation.md`;
  establish the actual deployed revision and source owner before copying.
- For alternatives and prior evidence, read
  `specs/research/2026-11-20-bb-home-lab-usage-audit.md` and
  `specs/research/source-provenance.md`.
- For implemented boundaries, read `specs/architecture/current.md`. Reconciliation
  rollout remains owned by its existing candidate/plan; verify its current state.

## Execution and verification

Use the home-lab skill before choosing or operating a host/service. Discover current
endpoints, capabilities, constraints, ownership, dependencies and recovery from the
central registry; verify relevant live state. Keep records there and procedures in
the skill. Inventory observations and authoritative capacity reservations are distinct.
Catalog writes alone do not authorize provider mutations.

Develop service changes in a disposable VM against actual registry/host/service
interfaces, with dedicated least-privilege staging credentials. Preserve stable code,
configuration and user data. Copying an existing secret requires explicit authorization.
Exercise actual browser interactions for UI changes and real lifecycle/failure recovery
for controllers. Present working staging; merge/promote service changes only after
explicit approval of that candidate. Promotion includes production verification,
preserved evidence/rollback artifacts, and cleanup of exactly owned staging resources.
Documentation-only commits do not deploy or promote services.

Third-party installation/migration follows its authorized backup, validation and
rollback plan; preserve already granted authority and explicit cutover requirements.
Respect task ownership, review holds and shared dependencies. Missing observations
do not authorize deletion. Verify backups, rollback and independent recovery before
changes that could disrupt access or data. Prefer DHCP and verified reservations.

## Publication and documentation

Keep credentials, real inventory, internal thread locators, private deployment exports,
personal data and recovery archives out of public Git and logs. Publish reviewed
source, redacted research, placeholders and secret references. Keep plans/research
in `specs/`; plans remain active until required work is verified. Update implemented
architecture only with evidence. Before committing docs, check links, publication
exposure, consistency and `git diff --check`; prose checks are not runtime validation.
