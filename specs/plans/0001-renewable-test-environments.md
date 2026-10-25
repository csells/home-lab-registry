# Renewable test environments as a home-lab service

Status: proposed implementation plan; documentation requested, implementation and
production promotion not performed. Created 2026-10-19.

Context: [vision](../vision/home-lab.md), [current architecture](../architecture/current.md).

## Outcome and agreed requirements

An agent derives resource needs from the user's task and acquires an environment
through one discoverable service endpoint. The user need not specify OS, RAM, GPU
or duration. The service handles safe allocation, access, registration, renewal,
recovery and cleanup. Environments support interactive work and independent jobs;
they do not require the coding agent itself to run inside them.

An environment belongs to an ongoing task and is reused across tests. The agent
chooses an initial lease and can renew it autonomously: 45 minutes can become 45
days. There is no arbitrary maximum lifetime for actively maintained work. Capacity,
ownership and permission limits remain enforced. Resize can be in-place where
qualified or a replacement operation; it must preserve saved work and report impact.

If an agent misses renewal, it can reconnect to a retained environment or create a
matching replacement. Evidence is accessible whether the environment is running,
stopped, expired or deleted, subject to a separate published retention policy.
Export evidence and checkpoints during work, not only during teardown.

## Scope and boundaries

Keep the registry as the sole discovery and inventory authority. Add typed task,
environment, lease and operation data rather than growing machine runbook blobs.
The service's durable operational state has one authoritative store; catalog views
are projections with explicit freshness, not a competing inventory. Decide exact
database placement before implementation and document its failure/restore semantics.

Use a small deterministic controller and qualified hypervisor adapters. Reuse
existing identity, backup and access infrastructure. Evaluate maintained components
before writing lifecycle machinery. No new cluster orchestrator is assumed.

Begin with one Linux template and a bounded test pool on a qualified worker. Fixed
slots are an implementation option, not a permanent limit on resource shapes or
environment lifetime. Later adapters cover flexible placement, GPU, Windows and
Apple-hosted testing separately. Production services are outside disposable deletion
scope. Existing authorized shared-service interfaces remain usable by test guests.

## Durable records

| Record | Required meaning |
| --- | --- |
| Task | Stable ID, owner/principal, originating work reference, lifecycle and retention policy |
| Recipe | Versioned image/template identity, architecture, capabilities, setup, source revision, input references and secret references only |
| Environment | Immutable allocation identity, task/recipe references, actual host/guest, resource bounds, readiness and lineage |
| Lease | Owner, generation, renewal deadline, grace policy, delegated job/review ownership and last renewal evidence |
| Operation | Idempotency key, requester, intended action, durable phase, upstream task IDs, result/error and reconciliation state |
| Evidence/checkpoint | Task/environment IDs, timestamps, checksums, completeness, export acknowledgement, retrieval and retention references |

Preserve uncommitted changes and declared working data through checkpoints, not
only Git commits. Recipe matching means recorded setup and capability constraints,
not identical IP, host or VM ID. No promise of restoring unsaved memory, arbitrary
process state, deleted external dependencies or bitwise GPU reproducibility.

## Proposed agent contract

Names below describe semantics; they are not existing routes or commands.

| Operation | Behavior |
| --- | --- |
| Discover | Return qualified capabilities, constraints, templates, capacity freshness and API schema |
| Acquire | Accept task, requirements, initial duration and idempotency key; return admitted/queued operation with reason |
| Inspect | Return readiness, actual allocation, deadline, export freshness, constraints and pending operations |
| Renew | Extend owned work without routine human approval; preserve the environment and return the new deadline |
| Resize | Plan and perform an authorized capacity change or matching replacement; report interruption requirements |
| Checkpoint / evidence | Export and acknowledge durable data; retrieve it independently of guest availability |
| Resume | Reconnect safely or reconstruct from recipe and latest acknowledged checkpoint, reporting gaps |
| Release | Export, revoke access and reclaim exactly owned resources; return verified completion or a concrete blocker |

Return structured error codes, retryability, operation IDs and safe next actions.
Retries with the same idempotency key must not create duplicate allocations. Auth
identifies the task principal, not a shared generic writer. A replacement gets a
new environment identity and credentials. Ownership transfer is explicit.

## Renewal, interruption and expiry

Lease renewal asserts continued ownership, not continuous CPU activity. Idle review
environments can be legitimate. The agent can delegate renewal to a registered job
runner or orchestration integration so an independent build or paused conversation
does not require inference to stay alive. Delegation is task-scoped and bounded by
its declared work/review policy; a VM heartbeat alone cannot renew forever.

Normal lifecycle: requested -> queued -> provisioning -> ready -> releasing ->
released. Renewal keeps a ready environment ready. A missed deadline enters grace
and ownership reconciliation, not immediate deletion. Then retain or suspend safely,
or reclaim according to the admission-time policy. Suspension is capability-specific;
do not assume all GPU jobs can be snapshotted or interrupted without loss.

Renewal during grace cancels eligible pending reclamation atomically. Once a
destructive operation is dispatched, renewal cannot pretend that destruction was
cancelled: finish reconciliation and offer a replacement. Active work is not
preempted merely to satisfy another request; queue or negotiate alternatives.

Expiry policy must expose grace, notification, maximum abandoned-data retention,
and what happens if export fails. Freeze/quarantine uncertain allocations, retain
bounded recoverable state where possible, and surface a blocker. Never claim export
success or silently delete the sole evidence. There must be an explicit disposition
for persistent export failure; indefinite hidden resource retention is not a policy.

## Evidence and recovery

Logs, commands, test results, failures and artifact references are associated with
the task and individual environment generation. Capture incrementally with a
bounded local spool and a declared export interval/maximum expected loss window.
Show last successful export, missing ranges, truncation, quotas and failed collection.
Test checkpoints for consistency; a tar of changing files is not necessarily resumable.

Integrate durable retention with the existing central encrypted Google Drive
destination. A service-managed working index/cache may serve retrieval, but must be
rebuildable and must not become a second retained backup destination. Qualify upload
latency and retrieval while the guest is absent; choose batching from measurements.
Keep secrets out of captured command lines, logs and public manifests. Apply scoped
artifact access and explicit redaction; never copy host credentials into a guest.

Resume verifies the recipe and artifact checksums, provisions matching capabilities,
restores the latest complete checkpoint, reacquires scoped secrets, and reports the
last captured point and unavailable dependencies. Retain predecessor evidence and
link the replacement. Evidence retention is independent of lease expiry; finite
retention and deletion dates must be visible, not implied to last forever.

## Safety and uncertain outcomes

- Admit resources atomically against protected household headroom and existing
  leases; use live hypervisor verification, not just a green health indicator.
- Route routine mutations through an executor scoped to a designated test pool.
  Ordinary task sessions receive guest access, not host-root credentials. Human
  recovery remains separate. Demonstrate denied access to protected guests.
- Prefer DHCP. Verify any required LAN reservation with the router before assigning
  a static address. Distinguish private isolated guest networks from household LAN.
- Persist intent before remote mutation. A timeout may mean success: reconcile by
  immutable ownership tags and upstream operation ID before retry or release.
- Validate generation/ownership at the mutation boundary. Generation checks cannot
  cancel already dispatched hypervisor tasks. Never reuse an allocation or slot
  while an earlier mutation is running or its result is unknown.
- Serialize renewal/release/reset transitions. On controller restart, reconcile
  operations and real guests before admitting reuse. Preserve leases, fencing state
  and operation identity through restore; stale database recovery requires fleet
  reconciliation before mutations resume.
- Controller/registry outages freeze unsafe new mutations; they do not shut down
  household services or interpret every guest as abandoned.
- Release capacity only after verifying removal/reset of owned disks, credentials,
  routes and reservations. Preserve shared templates and unrelated dependencies.

## Worked failure and recovery trace

1. Acquire records task T, request K, environment E and generation G; capacity is held.
2. A guest create succeeds but its response is lost. E is outcome-unknown, not free.
3. Controller restart finds E by immutable ownership and reconciles its upstream task.
4. The agent runs many tests in E and renews repeatedly for weeks. Evidence exports
   continue during work and remain readable independently of E.
5. The agent disappears; grace and ownership checks run without inference. Policy
   eventually releases E after acknowledged export and verified cleanup.
6. The agent returns: resume T creates matching E2 from the saved recipe/checkpoint,
   reports the capture gap, and exposes E's logs and results.
7. A delayed delete for E/G is rejected and cannot target E2. An in-flight old task
   must already have reached a verified terminal state before any slot reuse.

## Implementation sequence and acceptance

- [ ] **Contract and recovery policy.** Select maintained components/adapters;
  define typed schemas, authentication, durable state ownership, expiry/retention
  defaults, capture scope, quotas and API compatibility. Resolve policy choices
  below before enabling automatic deletion.
- [ ] **One isolated lifecycle.** In a disposable development VM, implement acquire,
  inspect, renew and release against a bounded Linux test pool. Reuse one environment
  for multiple tests. Prove idempotency, protected capacity, scoped access, router
  compliance and automatic cleanup. Development execution stays off stable.
- [ ] **Durable evidence and resume.** Exercise periodic export, uncommitted-work
  checkpointing and retrieval after guest deletion. Recreate a matching replacement
  from a real failed task and independently verify its saved result/work.
- [ ] **Failure qualification.** Concurrent requests exceed capacity without
  oversubscription; controller/requester crashes, lost success responses, export
  failure and delayed cleanup cannot duplicate or destroy another allocation.
  Interrupt an actual asynchronous reset and prove reuse waits for completion.
  Use a controlled clock to exercise weeks of renewal without weeks of waiting,
  plus a real restart/integration run. Controller restore reconciles stale state.
- [ ] **Agent and dashboard integration.** A fresh agent completes a user-level task
  without manual provisioning instructions; reconnects after interruption; sees
  queue reasons, renewal and evidence freshness. Human UI shows ownership and
  review retention, works on mobile and rejects unauthorized access. No recurring
  inference is used to keep leases alive.
- [ ] **Review and promotion.** Present functioning staging, real lifecycle evidence,
  overhead measurements and rollback/recovery artifacts. Obtain explicit approval
  for this candidate before promotion. Verify stable behavior, preserve source and
  recovery artifacts, then remove dedicated staging resources and retire records.
- [ ] **Documentation completion.** Update current architecture with implemented
  boundaries and validation, update the home-lab skill to use the service, and
  archive this plan only when required work is complete. Track later platform
  adapters separately rather than implying they were qualified by the Linux slice.

## Policy choices to settle during design

These do not block writing this plan. They do block an implementation from silently
inventing destructive behavior: grace and abandoned-data retention defaults;
review-hold ownership; capture exclusions and artifact quotas; disposition when
exports remain impossible; and restart-safe job delegation. Offer reasonable
defaults together for review, preserving autonomous renewal and no fixed active
lifetime cap. Automatic reclamation applies only to environments admitted under
that policy, never retroactively to existing workloads.

## Success measures

Measure intervention-free task completion, queue/acquisition latency, renewable
environment reuse, export lag and data-loss window, reconstruction time, orphan
disk/resource count and controller overhead. Require zero protected-service
mutations, zero cross-owner cleanup and zero unreserved static LAN assignments in
qualification. A matching-environment claim requires a real restored workload,
not just a successful clone command.
