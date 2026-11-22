# Renewable test environments as a home-lab service

Status: active, updated 2026-11-22. Two-host Linux leasing, on-demand clones
and dashboard integration passed staging at `e86592f`. Subsequent staging
verified one unattended Tart Xcode recipe, portable task access and replacement
after a Linux lease expired. Cross-runtime acceptance, a full controller restore
and managed CUDA remain open. Production remains on hold.

Context: [vision](../vision/home-lab.md), [current architecture](../architecture/current.md).

## Outcome and agreed requirements

An agent derives resource needs from the user's task and acquires an environment
through one discoverable service endpoint. The user need not specify OS, RAM, GPU
or duration. The service handles safe allocation, access, registration, renewal,
recovery and cleanup. Environments support interactive work and independent jobs;
they do not require the coding agent itself to run inside them.

Any authorized agent should be able to start from the home-lab skill, discover
an available service in the registry and use its documented interface. The
allocator's HTTPS API, standalone client and task credential contract apply
across runtimes. Set up lab access once through the documented credential
path, then give each authorized agent the same skill and scoped credentials.
Shared services retain their own documented authentication and grants.

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

## Placement and component choice

The current recommendation is one allocation/lifecycle authority over existing
Proxmox APIs and a first-class Apple worker. Native Proxmox clustering is a separate
management/migration decision, not a prerequisite for acquiring environments.
Evaluate maintained components before writing lifecycle machinery. Cloudpods should
replace custom work if its actual OSS deployment satisfies the same contract more simply.
See the [usage audit and architecture comparison](../research/2026-11-20-bb-home-lab-usage-audit.md).

Requests describe OS/architecture, isolation, versioned toolchain, CPU/guaranteed
memory/disk, qualified hardware access mode, dependencies, ownership and lifetime.
A request may span complementary Linux and Apple environments. Return actual
capabilities, ready access, dependency bindings, artifact location and lease status.
Prefer qualified Linux workers for ordinary Linux work to preserve Apple capacity.

Keep reusable controller/provider source here, with deployment policy and credentials
supplied privately. Registry catalog writes do not authorize provider mutations.
Workload frameworks consume this authority or a bounded delegated pool, rather than
maintaining competing physical-capacity ledgers. Account for out-of-band guests and
stopped disks; stale or unavailable observations do not establish free capacity.

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
| Acquire | Accept task, requirements, initial duration and idempotency key; return an admitted operation or reject with a reason |
| Inspect | Return readiness, actual allocation, deadline, export freshness, constraints and pending operations |
| Renew | Extend owned work without routine human approval; preserve the environment and return the new deadline |
| Resize | Plan and perform an authorized capacity change or matching replacement; report interruption requirements |
| Checkpoint / evidence | Export and acknowledge durable data; retrieve it independently of guest availability |
| Resume | Reconnect safely or reconstruct from recipe and latest acknowledged checkpoint, reporting gaps |
| Release | Export, revoke access and reclaim exactly owned resources; return verified completion or a concrete blocker |

Return structured error codes, retryability, operation IDs and safe next actions.
Retries with the same idempotency key must not create duplicate allocations. Auth
identifies the task principal, not a shared generic writer. A replacement gets a
new environment identity and credentials. Ownership transfer is explicit. A user-approved cleanup override must expose affected
consumers and data disposition. Existing workloads do not retroactively acquire a
destructive expiry policy.

## Renewal, interruption and expiry

Lease renewal asserts continued ownership, not continuous CPU activity. Idle review
environments can be legitimate. The agent can delegate renewal to a registered job
runner or orchestration integration so an independent build or paused conversation
does not require inference to stay alive. Delegation is task-scoped and bounded by
its declared work/review policy; a VM heartbeat alone cannot renew forever.

Normal lifecycle: requested -> provisioning -> ready -> releasing ->
released. Renewal keeps a ready environment ready. A missed deadline enters grace
and ownership reconciliation, not immediate deletion. Then retain or suspend safely,
or reclaim according to the admission-time policy. Suspension is capability-specific;
do not assume all GPU jobs can be snapshotted or interrupted without loss.

Renewal during grace cancels eligible pending reclamation atomically. Once a
destructive operation is dispatched, renewal cannot pretend that destruction was
cancelled: finish reconciliation and offer a replacement. Active work is not
preempted to satisfy another request. Reject when capacity is exhausted and give the reason.

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

## Mac worker delivery

Use an authenticated host-side executor for authoritative inventory and qualified clone/start/stop/delete/console operations. Begin with existing verified Tart images and concurrency constraints; increase concurrency only after measured real simultaneous workloads. Orchard or another maintained backend may supply worker functions; its resource accounting must have a clear authority boundary. Qualify Lume separately if strict OSS is required.

Offer Apple build/test guests, GUI-test guests, and explicitly exclusive native/device sessions. A macOS VM does not imply real audio, camera, USB or physical-device support. Keep Linux builds and ordinary test infrastructure on qualified Linux workers and bind authorized shared voice/model services.

Use the Mac's existing Xcode Simulator for ordinary iPhone and iPad testing
when the host and workload are authorized. Boot a dedicated simulator on demand
and shut it down after the task. Use a macOS guest when the task requires a clean
OS or toolchain, an isolated automation identity or a guest lifecycle test.
Qualify native admission and concurrency before offering it through the allocator.

Maintain versioned Xcode/simulator/Metal recipes and stable signed automation identities. Where an existing stable signing identity and scoped service helper are installed,
verify and reuse that boundary; new deployments must establish their own identities. Validate actual console capture/input and the requested app test on fresh clones; permission flags alone do not prove operation. Prefer a qualified virtual-console path where appropriate to avoid per-agent screen-capture permissions on the physical host.

Target routine acquisition without fresh infrastructure-consent dialogs after deliberate worker/image setup. Preserve explicit consent testing as a separate app-test case. Evaluate MDM only for supported policy/app/OS combinations; macOS 27 AppKit permission defaults still require consent and do not silently grant every CLI or screen-capture permission. Host GUI login, physical-device access and cold recovery remain separately qualified capabilities.

## Inventory integration carried from October

Reuse authenticated collector guest identity/state with observation times and expiry. Bind services/environments to exact host and guest identities; preserve legacy structured IDs during transition. Distinguish health, inventory, readiness and admitted capacity.

Report unregistered/missing guests, stale collectors, expired leases, missing bindings and duplicate identities without inferring deletion or purpose from process names. Multiple services may legitimately share a guest. Keep historical snapshots as evidence rather than a competing current view.

Use cached initial reports, change events and freshness heartbeats through the existing stream/SSE work. Avoid recurring central fleet polling or inference-based housekeeping. Local sampling versus OS-native event sources and registry metadata-change feeds must be described honestly. Manual reconciliation refreshes intent/observations without silently retiring workloads.

Production guest streaming and reconciliation shipped under
[plan 0003](archive/0003-inventory-reconciliation.md). General inventory coverage
and registry change delivery continue in [plan 0005](0005-inventory-coverage-and-change-delivery.md).
The lease review build joins dynamic clones to the controller projection under
this plan, while the controller remains the authority for lease mutations.

## Apple and cross-provider acceptance

The bounded Linux lifecycle is the first delivery, not the whole project charter.
Follow it with one Apple recipe through the same task/operation/lease contract.
Two tasks contending for the Mac must receive owned execution and an immediate
capacity rejection without disturbing an unrelated VM. Review holds survive nominal expiry;
controller restarts preserve ownership and pending operations.

A fresh clone must complete a real Apple build and actual GUI interaction without
repeating infrastructure consent setup. Report unqualified physical audio/device
access separately. Preserve shared services and existing host sessions. Qualify
concurrency using simultaneous real workloads, not nominal host capacity alone.

Return approved shared voice/model/build bindings rather than duplicating runtimes.
Exercise uncommitted-work recovery and evidence retrieval after guest deletion.
Native clustering or hypervisor replacement needs a separate guest-data preservation
and authorized cutover plan. Windows/GPU/device adapters remain explicit later work.

### Staged milestone: one unattended Apple lease

Keep the qualified Proxmox path available and use the same allocator API and
durable ledger for one prepared Tart macOS/Xcode recipe in disposable staging.
The allocator owns admission, renewal, holds, checkpoint receipts and release.
The registry publishes capabilities and observations. The Tart adapter owns
guest operations, verifies exact provider identity and reports uncertain outcomes
without making the capacity available. Use one Apple slot initially and return an
immediate, actionable rejection if a second request cannot fit.

Use the previously qualified Tart image and give each lease its own task access
and workspace. Reuse the existing verification and encrypted storage path for
checkpoint readback, retention and exact release. Place a provider-specific
operation behind the adapter as the second backend needs it. Preserve the Linux
workflow and its safety checks through the change.

The agent starts with the home-lab skill and its private task credential. It
discovers the allocator through the registry, derives capabilities from the
work, and acquires and renews the environment itself. The operator supplies no
host name, VM ID, guest credential or cleanup instruction.

- [x] A fresh agent handles an ordinary request to run the iOS simulator tests
  for a change. It discovers and acquires the recipe, accesses the guest,
  runs a real test and verifies a checkpoint.
- [x] Interrupt the task or restart the controller. Verify reconnect or matching
  replacement from the last acknowledged checkpoint and check that a replay
  keeps the original operation identity.
- [x] Verify immediate contention rejection, exact release of the owned clone
  and workspace, and independent evidence retrieval after release.
- [x] Record every manual setup, access, recovery and cleanup intervention.
  The normal path requires zero operator interventions and preserves existing
  household workloads. Compare the result and elapsed time with the runbook.

Staging has exercised the same allocator API on Linux and one Tart Xcode
recipe. The Apple lease renewed during cloning, refused a competing request,
and retained its identity after a controller restart. The project ran a real
iPhone Simulator UI test. Verified export preceded exact clone and workspace
deletion. The ordinary task client then restored the acknowledged workspace
into a fresh Tart clone, verified its unfinished work, replayed the replacement
request, and recovered the workspace again after deleting that clone.

Separate control and capture credentials refused cross-role access. The
launcher supplied a scoped access manifest. A fresh BB task chose the Apple
recipe, passed one real iPhone Simulator UI test and recovered 14 working
files after release. Independent verification checked the input evidence,
workspace content and exact cleanup. The task recorded zero operator
interventions.

In one run, timestamps showed about 172 seconds from accepted lease to the
test screenshot and 291 seconds to verified release. The prior direct
Tart harness took about 191 seconds from its pre-clone marker to its screenshot
record. These runs used different timing boundaries and provide a reference
for a future repeatable latency benchmark.

### Next delivery sequence

1. **Shared access from the home-lab skill.** Verify that fresh authorized
   agents in different runtimes can start with the same installed skill and
   private credentials, discover a service, and use its registered interface.
   For the allocator, verify scoped access, owner continuity across restarts
   and credential refresh, no allocation before a request, and immediate
   rejection when capacity is exhausted.
2. **Recover an interrupted agent and failed workload.** Repeat the verified
   client interruption drill after terminating and restarting a coding-agent
   runtime. Recover an actual failed workload, its evidence and its unfinished
   work. Measure export lag, the acknowledged recovery point and remaining data
   loss. Restore a controller journal and reconcile an in-flight operation
   against the provider before allowing reuse.
3. **Qualify one managed CUDA recipe.** Discover a suitable registered worker,
   verify available GPU capacity and run a real CUDA workload through the same
   lease API. Verify immediate rejection of a competing request, durable
   checkpoint recovery and return of the GPU after release. Exercise a second
   lease to prove the resource is available again. Record the GPU access mode
   and driver/image requirements separately from the generic Linux recipe.

Use the [Mayfly review](../research/2026-11-21-mayfly-allocator-review.md) to
improve worker maintenance alongside these milestones. Version each recipe by
its image identity and bootstrap revision. Check storage compatibility and
network configuration before cloning, then verify the real task access path
before declaring readiness. Add a read-only maintenance plan that shows whether
a worker change requires a reboot or replacement, affected owners and required
overlap capacity. Refuse a replacement when its generations cannot safely
overlap and name the missing capacity. Keep leases and reclamation under the
same allocator authority. Evaluate ephemerd separately if a GitHub Actions
workload needs a bounded lab pool.

Windows and wider Apple recipe enrollment follow these milestones. Production
promotion remains on hold.

### Current qualification evidence

- A standalone runner discovered the real allocator from the registry,
  refreshed a scoped task credential, preserved its SSH identity and queried
  only its own leases. Revocation rejected its former credential. A distinct
  task credential could neither inspect nor release an occupied Apple lease
  and received an immediate capacity rejection. The earlier simulator run
  through the shared client remains the verified Apple result.
- The independent Proxmox drill checkpointed a Debian workspace, interrupted its
  test client and let the original lease expire. The controller exported and
  removed the guest and its disks. The same task recovered the acknowledged
  archive and restored the unchanged file in a matching clone in 35 seconds.
  Both leases released, and the isolated drill restored the normal grace
  policy. A full restart of a coding-agent runtime remains a separate check.
- Staging renewed a 45-minute lease to 45 days through the live API, then
  exercised six additional renewals with a controlled clock. The export worker
  verified failure notification, retry and recovery across restart. The scoped
  expiry test removed an eligible stored checkpoint and confirmed recovery
  access ended. Production owner routing and standing credential setup remain
  open.
- The actual Proxmox template and datastore passed a fresh DHCP and storage
  check. New code can pin recipe versions and estimate maintenance impact without
  provider writes. Independent image provenance and qualification of a new
  enrolled version remain open.
- The real CUDA LXC probe passed and its disposable container and disk were
  removed. The allocator has no enrolled CUDA provider yet.

## Implementation sequence and acceptance

- [ ] **Contract and recovery policy.** Select maintained components/adapters;
  define typed schemas, authentication, durable state ownership, expiry/retention
  defaults, capture scope, quotas and API compatibility. Proxmox, scoped TLS
  credentials, a SQLite ledger and the Linux expiry and capture policy below
  have staging evidence. Cross-provider schemas, quotas and compatibility
  qualification remain open.
- [x] **One isolated lifecycle.** In a disposable development VM, implement acquire,
  inspect, renew and release against a bounded Linux test pool. Reuse one environment
  for multiple tests. Prove idempotency, protected capacity, scoped access, router
  compliance and automatic cleanup. Development execution stays off stable.
  [Controller and safety tests](https://github.com/csells/home-lab-registry/blob/e86592f9c6af06db2f84d08397aee4bdf8f85618/allocator/test_lease_safety.py) and the
  enrolled staging controller record cover two independent hosts, DHCP,
  idempotent acquisition and scoped reclamation.
- [ ] **Durable evidence and resume.** Exercise periodic export, uncommitted-work
  checkpointing and retrieval after guest deletion. Recreate a matching replacement
  from a real failed task and independently verify its saved result/work.
  The staged [checkpoint and recovery implementation](https://github.com/csells/home-lab-registry/blob/e86592f9c6af06db2f84d08397aee4bdf8f85618/allocator/README.md#matching-replacement-and-workspace-restore)
  verified multiple acknowledged versions, encrypted Drive readback after
  actual guest deletion, and matching replacement with byte-equal workspace
  recovery. Keep the full failed-workload acceptance and the measured
  export/recovery window open.
- [ ] **Failure qualification.** Concurrent requests exceed capacity without
  oversubscription; controller/requester crashes, lost success responses, export
  failure and delayed cleanup cannot duplicate or destroy another allocation.
  Interrupt an actual asynchronous reset and prove reuse waits for completion.
  Use a controlled clock to exercise weeks of renewal without weeks of waiting,
  plus a real restart/integration run. Controller restore reconciles stale state.
  Staging recovered real Proxmox task replies, rejected stale journal and guest
  generations, and held disks on export failure. The latest backup restored
  a consistent SQLite journal and matched retained receipts. Long-duration
  renewal has staged evidence. A full controller recovery drill through
  active-operation reconciliation remains open.
- [ ] **Agent and dashboard integration.** A fresh agent completes a user-level task
  without manual provisioning instructions; reconnects after interruption; sees
  rejection reasons, renewal and evidence freshness. Human UI shows ownership and
  review retention, works on mobile and rejects unauthorized access. No recurring
  inference is used to keep leases alive.
  A fresh BB task discovered the pilot, ran on Debian, and retrieved its workspace
  after release. The staged browser verified active managed clones and their
  removal across all six configured collectors. Production credential onboarding,
  owner alert routing and end-to-end recovery after an actual agent-runtime
  restart remain open. The shared workflow uses the skill, the registry and the
  standalone client. The later runtime-specific BB integration was retired.
- [ ] **Review and promotion.** Present functioning staging, real lifecycle evidence,
  overhead measurements and rollback/recovery artifacts. Obtain explicit approval
  for this candidate before promotion. Verify stable behavior, preserve source and
  recovery artifacts, then remove dedicated staging resources and retire records.
  [Review CI](https://github.com/csells/home-lab-registry/actions/runs/45037022105) identifies the tested source,
  and the staging record holds browser, lifecycle and restored journal evidence.
  Comparative overhead and production verification remain open.
- [ ] **Documentation completion.** Update current architecture with implemented
  boundaries and validation, update the home-lab skill to use the service, and
  archive this plan only when required work is complete. Track later platform
  adapters separately rather than implying they were qualified by the Linux slice.
  The [architecture](../architecture/current.md#allocator-staging) now records
  this staging boundary, and the home-lab skill documents scoped discovery,
  renewal and recovery. The plan remains active through the open requirements.

## Agreed Linux operating policy

- Reject immediately with a specific reason when no eligible capacity is available.
- Allow a two-hour grace period after missed renewal. Reclaim enrolled resources
  only after verified export, unless an authorized hold applies.
- Keep verified checkpoints for 30 days after reclamation. Owners can pin work
  longer and see the retained-through date.
- Stop safely on export failure where possible, retain the disk, alert the owner
  and retry. After 24 hours of continued failure, require a recorded repair,
  retain or discard decision.
- Let the task owner or designated reviewer record a hold and reason. A running
  hold explicitly reserves live capacity. Legacy guests require separate enrollment.

## Success measures

Measure intervention-free task completion, admission and acquisition latency, renewable
environment reuse, export lag and data-loss window, reconstruction time, orphan
disk/resource count and controller overhead. Require zero protected-service
mutations, zero cross-owner cleanup and zero unreserved static LAN assignments in
qualification. A matching-environment claim requires a real restored workload,
not just a successful clone command.


## First implementation slice

Use the existing Proxmox API and a dedicated pool-scoped service identity for
Linux guest lifecycle. Qualify the template and clone, start, stop, task-status
and delete permissions against an isolated staging pool. Keep the controller's
durable operation journal separate from guest storage, and publish registry
records after each verified transition. The central encrypted backup system is
the existing destination to qualify for checkpoint export and restore.

The pilot started with explicit release and overdue owner review. The agreed
Linux policy now governs staged automatic reclamation. Staging verified
reclamation after expiry and verified export, plus a disk hold on export failure.
Wider delegation, operator routing and production deployment remain open.

## Staging evidence and provider work

At `e86592f`, one ledger governed a fixed slot and an on-demand pool on
independent Proxmox hosts. A fresh dynamically sized Debian clone grew its
root filesystem, survived controller restart and acquisition replay, and
exported its workspace before verified deletion of the exact guest and disks.
The same pool restored a matching replacement. The dashboard recognized
the live clone as a managed lease and cleared its row on release. The
registry's staging deployment record holds the qualification receipts.
[Provider source](https://github.com/csells/home-lab-registry/blob/e86592f9c6af06db2f84d08397aee4bdf8f85618/allocator/pve_dynamic.py) · [Passing CI](https://github.com/csells/home-lab-registry/actions/runs/45037022105)

Independent Tart checks exercised a fresh Swift build, iPhone simulator input,
Metal and encrypted workspace retrieval after clone deletion. The allocator
subsequently qualified one prepared Tart Xcode recipe as recorded above;
the independent Metal probe does not qualify a managed Metal recipe. A
disposable Linux CUDA container exercised actual GPU memory; managed CUDA
still needs authenticated provider operations and recovery. Windows
and physical-device adapters, wider pool enrollment, in-place resize of active
leases, and task grants for model and Bazel services remain open. The current
voice binding returned the service endpoint and relies on its own authorization.
