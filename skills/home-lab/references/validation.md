# Home-lab workflow validation

## Promotion cleanup — 2026-10-12

The operator explicitly made promotion approval include permanent cleanup of the
associated disposable staging environment. Global agent rules and both published
home-lab skill copies now carry that authorization.

Observed workflow: the reviewed registry release was pushed to main, production
browser checks and CI passed, and central configuration backup completed. The
agent verified the exact dedicated staging guest identity and stopped state,
retained source and verification evidence outside it, purged its disks and guest,
removed temporary client credentials, and verified the retired registry record.
Production remained the service endpoint; the old preview redirects there.

Policy boundary review (not an automated agent evaluation):

- Promotion approved and production verified: complete staging cleanup without
  another permission request; stopping the guest alone is insufficient.
- Production validation fails: preserve recovery artifacts and resolve the failed
  promotion before deleting staging needed for diagnosis.
- Shared dependency, template or unrelated guest: exclude it from cleanup.
- Explicit retention instruction or concrete dependency: record owner, reason and
  expiry, and complete independent cleanup.

Run `uv run scripts/skill_health.py --skill home-lab` for structural validation.
This check does not prove that future agents will follow the workflow.
