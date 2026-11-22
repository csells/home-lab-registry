# Renewable test environments as a home-lab service

Status: in progress. A bounded Linux pilot runs in disposable staging.
Created 2026-10-19; expanded 2026-11-20 with heterogeneous placement, Apple workers
and the usage audit. No allocator or provider migration is claimed deployed.

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
| Acquire | Accept task, requirements, initial duration and idempotency key; return admitted operation or immediate rejection with a specific reason |
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
released. Unsatisfiable requests are rejected immediately. Renewal keeps a ready environment ready. A missed deadline enters grace
and ownership reconciliation, not immediate deletion. Then retain or suspend safely,
or reclaim according to the admission-time policy. Suspension is capability-specific;
do not assume all GPU jobs can be snapshotted or interrupted without loss.

Renewal during grace cancels eligible pending reclamation atomically. Once a
destructive operation is dispatched, renewal cannot pretend that destruction was
cancelled: finish reconciliation and offer a replacement. Active work is not
preempted to satisfy another request. Return the limiting resource or capability so the caller can adjust or retry.

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

Maintain versioned Xcode/simulator/Metal recipes and stable signed automation identities. Where an existing stable signing identity and scoped service helper are installed,
verify and reuse that boundary; new deployments must establish their own identities. Validate actual console capture/input and the requested app test on fresh clones; permission flags alone do not prove operation. Prefer a qualified virtual-console path where appropriate to avoid per-agent screen-capture permissions on the physical host.

Target routine acquisition without fresh infrastructure-consent dialogs after deliberate worker/image setup. Preserve explicit consent testing as a separate app-test case. Evaluate MDM only for supported policy/app/OS combinations; macOS 27 AppKit permission defaults still require consent and do not silently grant every CLI or screen-capture permission. Host GUI login, physical-device access and cold recovery remain separately qualified capabilities.

## Inventory integration carried from October

Reuse authenticated collector guest identity/state with observation times and expiry. Bind services/environments to exact host and guest identities; preserve legacy structured IDs during transition. Distinguish health, inventory, readiness and admitted capacity.

Report unregistered/missing guests, stale collectors, expired leases, missing bindings and duplicate identities without inferring deletion or purpose from process names. Multiple services may legitimately share a guest. Keep historical snapshots as evidence rather than a competing current view.

Use cached initial reports, change events and freshness heartbeats through the existing stream/SSE work. Avoid recurring central fleet polling or inference-based housekeeping. Local sampling versus OS-native event sources and registry metadata-change feeds must be described honestly. Manual reconciliation refreshes intent/observations without silently retiring workloads.

The existing streamed-inventory candidate has validation history but its fleet rollout
and promotion must be checked live. Preserve its task ownership. Its separate
[reconciliation plan](https://github.com/csells/home-lab-registry/blob/277fbda89da4dcd1002d37665e3477ae93ad937e/specs/plans/0003-inventory-reconciliation.md)
retains the rollout work; this plan does not assume that candidate is stable.

## Apple and cross-provider acceptance

The bounded Linux lifecycle is the first delivery, not the whole project charter.
Follow it with one Apple recipe through the same task/operation/lease contract.
Two tasks contending for the Mac must receive owned execution and an actionable
rejection reason without disturbing an unrelated VM. Review holds survive nominal expiry;
controller restarts preserve ownership and pending operations.

A fresh clone must complete a real Apple build and actual GUI interaction without
repeating infrastructure consent setup. Report unqualified physical audio/device
access separately. Preserve shared services and existing host sessions. Qualify
concurrency using simultaneous real workloads, not nominal host capacity alone.

Return approved shared voice/model/build bindings rather than duplicating runtimes.
Exercise uncommitted-work recovery and evidence retrieval after guest deletion.
Native clustering or hypervisor replacement needs a separate guest-data preservation
and authorized cutover plan. Windows/GPU/device adapters remain explicit later work.

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
  rejection reasons, renewal and evidence freshness. Human UI shows ownership and
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

## Agreed operating policy

- Protect household headroom, active leases and explicit review holds. When no
  eligible capacity is available, reject immediately with a specific reason.
  The caller can change the request or try again later.
- A missed renewal starts a two-hour grace window. Reclaim automatically once
  grace ends and recovery data is verified, unless an authorized hold applies.
- Keep verified checkpoints and evidence for 30 days after reclamation. The
  owner can pin specific work for longer. Show the retained-through date.
- On export failure, retain the disk and free CPU/RAM when the guest can safely
  stop. Alert the owner and retry. After 24 hours of continued failures, require
  a recorded repair, retain or discard decision.
- The task owner or designated reviewer may record a hold and its reason.
  Retain the disk by default. A running hold explicitly reserves live capacity.
- Review legacy expiries once per guest with its owners. Preserve existing holds.
  Enroll retained guests only with their owners' acceptance of the new policy.

The controller must qualify restart-safe renewal delegation, bounded capture,
and export and recovery before enabling automatic deletion. Active owners
retain autonomous renewal without an arbitrary maximum lifetime.

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

Start with explicit release and an overdue state that flags owner review.
Enable automatic reclamation only after the agreed retention and export-failure
behavior and restart-safe delegation have been verified. No policy decision is
required to prototype acquire, renew, explicit release and interrupted
operation recovery in the dedicated test pool.

### Staging milestone: one pre-provisioned slot

A disposable controller uses SQLite to journal one pre-provisioned Proxmox
guest, with a provider identity scoped to that guest. Real staging verified
start and DHCP, inspection after a controller restart, idempotent acquisition,
immediate capacity rejection, renewal, explicit release with a real disk
reset, and reacquisition. An old lease release left a later lease untouched.
A missed renewal marked the lease overdue and kept the guest available for
renewal. An unauthenticated request and a read of a protected guest were denied.

Remaining for the Linux slice: shared reservation discovery and registry projection,
automated credential issuance, incremental checkpoints, and stable deployment.

The pilot now journals a pre-dispatch view of recent tasks. When the provider
reply is lost, it attaches only a unique new task for the same dedicated token,
node, guest and operation type. Staging discarded accepted replies for real
start, shutdown and rollback tasks before recording their IDs. Independent
controller processes recovered each task from Proxmox and verified that each
mutation executed once. An intent without a unique match keeps its reservation
for operator review. Broader provider and multi-slot recovery qualification
remain open.

The pilot now binds each authenticated request to its task principal and
installs that principal's public key only for the leased guest's non-admin
account. Live staging exercised two distinct principals. Cross-owner
inspect, renew, release and SSH failed. The replacement guest accepted its
own key and rejected the previous key after rollback. SSH strict host
verification continued across a DHCP address change. This was a staging
harness over a private operator transport; fresh agent discovery and client
credential issuance remain separate qualification.

The pilot checks live Proxmox memory, configured guest commitments, and
datastore headroom with a separate read-only token before dispatching a start.
Configured protected guests count toward commitments even when stopped.
Rejections for insufficient memory, insufficient storage, incomplete
inventory, and an unauthorized capacity read were tested against the real
host. Each left the lease journal and provider start history unchanged.
The pilot does not yet allocate across hosts or automatically account for
reservations held outside its ledger.

The staging client discovered the pilot in the real registry and completed
acquire, SSH work, capacity rejection, renew and release over private TLS
without using operator SSH or provider credentials. The operator supplied
the task token and CA. An independent coding-agent run and automated
credential issuance remain open.

A task-side capture contained a committed Git baseline, real uncommitted
changes and an evidence file. The central backup controller stored it in
encrypted Drive. After release and slot reset, a clean replacement loaded
the verified archive from Drive and recovered both the working diff and the
evidence. This was an operator-driven backup integration check. Incremental
exports, fully automated verification and restore, qualification after
physical guest deletion, and full reclamation qualification remained open at this milestone.


### Staging milestone: final export before reset

A new pilot lease can opt in to expiry reclamation. During grace, the original
owner can renew. Once grace ends the controller stops the guest, records the
export work and holds the disk. An explicit checkpoint-preserving release
uses this same path. A missing capture path produced a real failure and
left the stopped slot reserved. A running hold kept the guest running
beyond grace. A disk hold survived the verified receipt and a controller
restart. The owner reopened a failed export after an accelerated 24-hour
test, and the previous challenge was refused.

Using the real encrypted Google Drive backup, the verifier downloaded the
checkpoint and compared its digest before authorizing reset. Clearing the
disk hold let the controller finish the rollback without a task client
driving progress. A replacement started with a clean disk and recovered
both the unfinished work and the failure evidence from the checkpoint.
This milestone qualified reuse after destructive disk rollback.

A timer ran the export worker inside the staging VM. The test withheld
the first receipt after a successful upload and readback. The timer found
the already uploaded snapshot using the export challenge, read it back
again, and recorded the receipt without a second upload. Separate scoped
operator forwarding exercised the real stopped guest and backup repository
without storing provider or backup credentials in the worker.

Keep automatic reclamation scoped to the owned pilot slot. Stable still
needed durable restricted capture and backup credentials, incremental
checkpoints, independent owner retrieval, 30-day retention and pinning,
notification delivery, delegated reviewer holds, and
shared reservation coordination. The pilot does not enroll existing
guests. This candidate has no production promotion approval.


### Staging milestone: owner recovery, retention and delegated renewal

An owner used their task credential to obtain and redeem a short-lived
checkpoint ticket over TLS. Another owner could not obtain a ticket for
the lease and the first owner could not replay a consumed ticket. The
client stored the archive only after checking its digest.

An independently owned disposable clone was backed up and then removed
with its disks. The controller was restarted. The owner downloaded the
same archive after the clone was gone and recovered both unfinished work
and failure evidence in a matching replacement. The original staging
slot remained available.

A pinned snapshot survived an accelerated 30-day expiry and controller
restart. Unpinning allowed the verifier to claim exactly that snapshot.
The worker removed it from encrypted storage. After simulating loss of
the completion receipt, a retry confirmed that the exact snapshot ID
was absent. A separate checkpoint still restored intact.

A real systemd job renewed a lease with a separate renewal-only credential
after the controller restarted. The credential could not inspect, release
or renew a different lease. Expiry and revocation bounded its authority,
and the owner could still renew independently.

After an accelerated 24-hour export failure, the owner recorded a disk
retention decision. The controller refused early discard, an incorrect
lease confirmation and a different owner. The exact owner then explicitly
authorized discarding the isolated test fixture. The controller persisted
its decision with the reset intent. A replacement confirmed the disk was
clean. Its first admission was refused while the host capacity observation
was unavailable, and it started after a fresh capacity check.

Stable still needs dedicated scoped capture and backup transports,
shared reservation coordination, automated agent
credential issuance, notification delivery, and delegated reviewer holds.
The staged storage and recovery tests used short-lived operator adapters.
Broader provider delivery and production approval remain open.


### Staging milestone: versioned task checkpoints

Two versions of a live task workspace and its evidence were uploaded using
the ordinary task credential. Verification downloaded both from encrypted
Drive before acknowledgement. An interrupted client recovered the first
receipt by replaying its original request key. A different digest using
the same key was denied, and a mismatched upload was retried under the
same version ID.

The owner retrieved an older version while the test VM was active after
a controller restart. A deliberate failure of the final export left the
disk held and the last acknowledged version independently recoverable.
Cleanup of the owned test guest started the 30-day retention window for
both versions. A staged systemd timer uploaded two archives without
an inference session. The task runner controls when a consistent archive
is ready. The unpinned version expired under an accelerated retention
test while the other version remained restorable.


### Staging milestone: shared reservations and storage roles

Two orchestration principals used the same durable ledger as task admission.
A real reservation caused the controller to refuse a guest start before
and after restart. Releasing it allowed the start. Two concurrent requests
that could not both fit produced one reservation and one immediate storage
rejection. A second principal could neither inspect nor release the first
principal's reservation. This qualifies the fixed host and controller.

A separate encrypted Drive repository was initialized using the backup
service's existing storage interface. The scoped writer stored and verified
a task archive, the scoped reader downloaded it, and only the expiry role
could remove it. The expiry role confirmed the snapshot was gone. The
role test used a temporary operator transport against the dedicated
repository. Separate forced-command SSH identities on a disposable staging
VM exchanged an actual archive and rejected shell and cross-role commands.
The scoped storage adapter was authorized and installed on the backup
controller. Production SSH verified independent read, write and expiry roles
against a dedicated encrypted Drive repository. The production configuration
archive was downloaded and its recovery key reopened the repository. The staging allocator subsequently exercised this adapter with scoped cold
capture, as recorded below. Production allocator adoption remains outstanding.


### Staging milestone: short-lived task credentials

A scoped launcher issued a short-lived credential to a fresh client. The
client found the actual controller through the registry, acquired a real
guest, and used an independently generated SSH key to reach its non-admin
account. A second issuer using the same task reference could not see or
operate the first lease. Controller restart, rotation, expiry, revocation
and delegated renewal revocation were exercised. Stable job-launcher integration remains outstanding. The subsequent dashboard
milestone records browser acceptance.

### Staging milestone: unattended cold capture with scoped provider access

A fresh task wrote uncommitted work and evidence to its declared workspace.
The production backup controller's separate read and write roles served the
disposable staging allocator. A scheduled export used a dedicated SSH identity
on the Proxmox host, restricted to one stopped raw slot. The host checked the
guest ID, name, hardware UUID and volume under its config lock. Shell requests,
substituting a guest, and reading a running slot were denied.

The capture worker verified the exact raw size and opened the disk in a
read-only libguestfs appliance with networking disabled. A wrong lease marker,
a wrong VM identity, and an incomplete source failed before an archive was
returned. After an independent download from encrypted storage, the controller
reset the slot. The owner retrieved the final edit and evidence after reset.
The worker removed its temporary raw copy. In one test the scheduled job used
21.4 CPU seconds and peaked at 271 MiB; this does not establish limits for other
disk sizes. Recovery covers the declared workspace of the qualified ext4 recipe.

The scoped host installation and its pre-change state have independently
retrieved encrypted configuration backups. The staging credential expires and
is bound to the worker. A stable allocator deployment still requires review,
credential provisioning, production verification and staging teardown.

A read-only dashboard review shows actual lease owners and phases, holds,
export errors, available recovery points and retention dates. On the real
staging ledger, a task credential was denied the operator index and the
operator reader was denied allocation writes. The authenticated lease page
passed refresh and pagination checks at phone and desktop widths.

Staging admission reports the fixed recipe and measured vCPUs, memory and disk.
An ordinary task requested Linux by capability, reached the matched guest and
released it. Unsupported GPU, CPU and memory requests returned reasons without
creating a lease. An identical retry returned the same lease; changing the
requirements under the same key was refused.

A distinct task principal received a reviewer assignment, placed a running
hold and changed it to a disk hold. The owner clearing its own hold did not
clear the reviewer's hold. Restart preserved it, and allocation release and
reviewer replacement were refused. An unrelated task could not change the
hold, and the reviewer could not inspect the guest or release it. An explicit
owner override recorded the named reviewer, prior reason and effect before
the normal checkpoint-gated release returned the slot.

### Staging milestone: replacement of a released Linux lease

The owner recovered a verified final workspace into a matching replacement
using a new SSH key. The previous guest's key was refused. The test edited
the recovered file and replayed the original request. The client returned
the same lease and preserved the new edit. Scheduled final export and reset
returned the replacement slot to the pool. This run exercised final capture.

One fresh BB thread received a short-lived task credential and the client
bundle. It discovered the allocator in the registry, requested Linux by
capability and connected using the guest identity returned by the lease.
Python round-tripped a UTF-8 filename and a 260-byte binary payload in a
gzip-compressed tar archive. The agent waited for the asynchronous release,
downloaded the final checkpoint and verified its source and result. A second
download matched the archive digest and both copies of the payload. A
provider read confirmed the slot was stopped, the allocator had zero
unreleased leases and the temporary disk spool was empty. The credential
was issued for the test. Automatic credential handoff by a production job
launcher remains outstanding.

### Staging milestone: whole-dashboard review access and stream recovery

The operator limited terminal and password review to the disposable controller.
A separate Guacamole installation connected a browser to its non-admin account.
A command typed in the browser returned the expected username and hostname.
The clipboard value matched the configured credential and authenticated to the
terminal login. A different machine and a foreign origin were refused.

A browser with its inventory stream interrupted reproduced a manual comparison
that completed on the server but stayed pending on screen. The updated UI reads
the cached comparison result while that user request is outstanding. A second
test verified automatic reconnection after an HTTP failure closed EventSource.
These checks used the actual registry, collectors and staging ingress. Public
owner-session observations are recorded in the private deployment record.

### Staging qualification after release review

New leases use checkpoint-gated reclamation by default. The operator enrolled
the idle Linux slot with its exact rollback, disk identity and a controller
ledger witness. A stale lease generation could not stop the running task.
A pre-renewal copy of the actual ledger also failed the witness check while
the VM generation was still current. The reconciler reads all-owner provider
history using the separate node audit identity.

A test guest containing 20,001 empty files passed its two-hour grace boundary
using an accelerated test deadline. The worker stopped the guest and rejected
the compressed archive under the guest restore limits. It held the original
disk until the test owner exercised the explicit failed-export disposition.
A separate small workspace round-tripped through encrypted storage, release
and a matching replacement. The replacement retained its recovery point
across a simulated retention deadline and acknowledged the restored file.
A stale cached SSH address was corrected from the live guest agent.

The authenticated staging dashboard copied the disposable login credential,
opened its browser terminal and completed a comparison across all configured
collectors. A different machine and a foreign-origin copy request were denied.
The recovery count uses the same eligibility rule as retrieval.

An interrupted verification after a real encrypted backup left the source disk
held. Explicit retry rotated the verifier challenge, reused the stored snapshot
under its original storage tag, completed a fresh readback and released the slot.
An inventory of the dedicated repository after retry matched every physical
snapshot to an acknowledged receipt.

For a separate upload, storage wrote the test archive before the worker reported
an interrupted acknowledgment. After release the operator advanced eligibility
in an isolated copy of the actual ledger. The existing off-VM expiry role
removed exactly the unacknowledged snapshot from the dedicated repository.
The independently verified final checkpoint remained available. The dashboard
also excluded a verified final export from its downloadable count during a
disk hold.


### Staging milestone: export-failure notifications

A failure recorded by the regular export worker held the leased disk and
persisted an alert in the same ledger transaction. A separate scheduled worker
could read and acknowledge alerts. The normal task and export-verifier
credentials could not read the alert queue. The alert worker could not
inspect capacity or claim an export.

A deliberately unavailable staging receiver left the alert pending. Restarting
the controller preserved it. The worker retried into an isolated staging inbox,
acknowledged one delivery, and did not submit a duplicate on its next run.
Normal capture subsequently verified the checkpoint and released the slot.
Production owner routing and production launcher integration remain open.

### Staging milestone: provider placement and Apple recipe

The placement preflight matched an ordinary Debian request against fresh
capacity on two independent Proxmox hosts. It rejected oversized requests
and expired observations immediately. The selected host passed a disposable
clone, actual task SSH, generation-bound registration and exact teardown. The
fixture exercised operator provisioning. The shared controller now admits
prequalified slots on independent Proxmox nodes. The on-demand Debian
profile below uses the same authority.

The existing Apple recipe compiled and ran a native Swift program in a fresh
Tart clone alongside an existing review VM. XCUITest built and launched an
iPhone simulator app, tapped a button, and asserted its changed accessibility
label. The native Metal test compiled a compute kernel, ran it on the Apple
paravirtual device, and compared the output buffer with expected values.
The test clones and their registry records were cleaned up after each run. Admission for these checks
used measured host headroom. A host-side GUI session, external device access,
and an allocator-managed Apple lease remain separate acceptance requirements.

### Staging milestone: shared dependency bindings

The Linux pilot advertises the names of operator-qualified shared
dependencies. A task may require them when acquiring a lease. An unknown
name is rejected before provider start, retry preserves the allocation,
and lease inspection returns only the requested references. A real guest
reached the bound voice service's HTTPS health check. Its protected
catalog still required its own authentication. Model and build service
bindings and task-level application credentials remain open.

### Staging milestone: one authority across two Proxmox hosts

A disposable controller migrated its existing ledger and receipts and
enrolled a second prequalified Proxmox slot. The two slots use independent
capacity observations, exact provider identities, and separate scoped
capture paths. Each lease records its slot before provider start. The
controller refuses excess demand with the reason and does not queue it.

Simultaneous requests with separate task credentials occupied both slots.
The read-only lease review counted both, SSH used separate pinned task
keys, and cross-owner inspection failed. After a real controller restart,
repeating the original request keys returned the existing slot, lease and
generation. Both host-specific reservation paths and oversize rejection
were exercised through the normal API.

After release, both original workspace files were downloaded and compared
with the task inputs. With the predecessor’s slot occupied, the usual resume
command restored the same acknowledged checkpoint on the other host. A retry
returned the original replacement and kept the acknowledged recovery point.

The original two-host run used pre-provisioned Linux slots. The on-demand
profile below extends that design with bounded CPU and RAM sizing.

### Staging milestone: on-demand Debian allocation and deletion

The disposable controller provisioned a fresh Debian guest through a
dedicated Proxmox pool. An oversized request was refused before cloning.
The actual guest used its own DHCP lease, pinned task key, two CPUs and
768 MiB of configured RAM. Restart and request replay preserved its
clone intent. The worker captured and verified the declared workspace
before deletion. Independent host-side inventory confirmed physical
volume removal, and the owner recovered the same bytes after deletion.

The staging run exposed two provider constraints. Proxmox can filter a
storage listing by permission, and deletion removes the guest-specific
ACL. The stage now checks its dedicated storage through a read-only
physical inventory and grants allocation through the dedicated pool.
The same pool recreated a matching guest, restored its acknowledged
workspace and returned the same replacement on request replay.

A separate run exercised a 9 GiB request through the normal lease API.
It verified both the guest block device and the grown root filesystem.
The stage admits bounded disk sizes from 8 through 12 GiB and records the
resize before sending it to Proxmox. The exporter verifies the admitted
disk length.

Guest inventory joins each observed clone to the controller's active lease
using the configured registry host, guest ID, provider type and exact
lease-specific name. The dashboard checks the read-only lease projection
when those guest observations change or when an operator runs a comparison.
It flags unavailable lease ownership separately. This comparison does not
publish a second lifetime record or authorize deletion. Further dynamic pools
remain open.

The disposable dashboard showed the running clone as a managed lease and
opened the Leases tab from its row. After release, the row disappeared from
the authenticated host inventory. Both checks verified all six configured
collectors and found no discrepancy for the owned clone. The task credential
was denied access to the operator lease projection.

### Staging milestone: Apple and CUDA provider groundwork

A disposable Tart guest wrote to a workspace shared from a dedicated
directory on the actual Mac host. The archive rejected a link that
escaped the workspace. After the clone was deleted, encrypted Drive
readback matched the archived bytes. This qualifies a workspace export
path for Apple guests and leaves their lease integration open.

A fresh Linux CUDA container allocated, wrote and read back GPU memory
on the registered CUDA host. Its network interfaces were removed before
boot. The proof used the existing template, protected other running
guests, and removed the container, disks and reservation afterward.
CUDA admission and lifecycle through the allocator remain open.

The private model service accepted a catalog read through a temporary
authenticated relay from the disposable stage. Its service listener stayed
on loopback and the relay was removed. The build service responded to a
health check inside its VM. Ordinary task grants for model and build use,
Windows and physical-device adapters, and wider rollout remain open.
