# Home-lab platform decision from actual BB usage

2026-11-20. Public, redacted synthesis of read-only research and local conversation
inspection. Internal thread IDs, host identities, guest IDs, private project/source
locations and deployed resource inventory are omitted. This is historical evidence,
not an independently reproducible benchmark or a statement of current availability.
No guest, service, network, profile or permission changes were made for the audit.
Owner-controlled original evidence remains outside public source history.
[Provenance and consolidation](source-provenance.md).

## Recommendation

**Implement one allocation/lifecycle authority over the existing Proxmox and Apple backends. Make the Mac a managed worker first. Treat native Proxmox clustering as a separate management/migration decision, not a prerequisite for acquiring environments.**

Among the three requested alternatives, rank Proxmox plus one allocator first, Cloudpods over Proxmox second only if its adapter materially reduces the implementation, and a native Cloudpods rebuild as an experimental challenger. This ranking comes from observed failure modes, not migration aversion. The strongest evidence concerns missing ownership, admission, recipes and Mac execution identity; it does not establish Proxmox as the source of those failures.

The main counterargument is substantial: a bespoke allocator is a permanent critical service, not a cheap wrapper. Cloudpods should win if its actual OSS build supplies the required contract with less custom lifecycle/recovery work. Compare working outcomes, rather than assuming either product or custom code will be simpler.

This supersedes the earlier feature-first preference for native Cloudpods in this research series. The user's willingness to migrate remains fully accepted; the usage evidence changes which experiment has the highest expected payoff.

## Scope and evidence quality

- Listed 338 BB thread records, including archived/hidden entries, in the CLI's bounded listing. This is not a claim that the listing exhausts every historical thread.
- Searched active and archived history using 14 broad terms and four follow-up query forms. Broad terms: Tart, Proxmox, Screen Recording, capacity, staging, Xcode, TCC, codesign, permission popup, cloud-init, VMID, cleanup, reservation and a host-name term. Search results are bounded and sometimes fuzzy; hit counts were not used as failure-frequency statistics.
- Opened 28 retrievable anchored conversation groups across 20 distinct threads, plus recent timeline context for platform, security, audio and acquisition work. The current architecture discussion was excluded as evidence of past usage.
- Read the current registry catalog and relevant Mac resource/runbook fields. Read the local maintained registry repository's vision, current architecture, renewable-environment plan and source layout.
- Three directly requested message anchors returned 404 after search/context had exposed historical text. One orchestration-design anchor was excluded entirely. Two human instructions were visible in successfully retrieved adjacent conversation context. Internal locators are retained only in owner-controlled evidence. Do not count those 404s as independent coverage.
- Two delegated readers could not access their tool runtime. They inspected no threads. Their later assessments used excerpts the root agent had already read; they provide reasoning review, not additional source coverage.
- These are historical human/agent records, not fresh reruns of every workload. An agent saying a VM crashed does not establish a kernel panic; a reported test success is recorded as such unless corroborated by the registry or artifact. No claims of fleet-wide incident rates or total time savings follow from this sample.
- Transcripts were read locally. Credential-bearing passages were excluded from the report; raw transcripts are not research attachments or external search inputs.

## What agents actually do

| Observed use | Evidence | Implication |
|---|---|---|
| Cross-platform app qualification uses Linux and Apple environments in parallel | A cross-platform application qualification allocates separate Linux and macOS/Xcode guests; later work resumes one platform and boots a distinct Apple worker. | The request should describe platforms/toolchains and scarce capabilities; it should not require the agent to learn both hypervisor runbooks |
| Ordinary Mac work repeatedly waits on another thread's VM | One application finds another owner; another passes checks but staging is blocked by the shared-worker serialization rule. | Replace repeated permission conversations with an authoritative queue and owned allocations |
| Agent-visible VM state can be misleading | A GUI application task first reports a stopped guest, then corrects itself after an authoritative check finds it running and requests a second guest. (including adjacent retrieved context) | The allocator should execute inventory checks on the actual host; an agent's sandbox/process view must not determine ownership or availability |
| One thread's approved cleanup invalidates another thread's test installation | A cross-platform task discovers its Linux VM reclaimed under approval elsewhere; source/evidence remain but installed test environment is gone. | Make active consumers/owners and retention visible before cleanup; support explicit user override rather than silently treating every temporary guest as disposable now |
| PVE guest provisioning already works | One Linux application reaches cloud-init completion, DHCP, pinned SSH and its required GUI/runtime. A speech application reaches its matching tooling release and real browser audio playback. | The repetitive work is orchestration around successful primitives, not demonstrated inability to run useful workloads |
| PVE teardown works, but is repeatedly agent-authored bookkeeping | Website validation is followed by deletion of staging VM and disks and completion of records. | Turn a repeatable operation into a lifecycle primitive with verifiable completion |
| Heavy builds require actual capacity | A build task reports Go/Bazel qualification peaking at roughly 20–24 GB and an OOM in a roughly 15 GiB guest despite swap; later capacity changes are operational inventory, not a portable sizing guarantee. | Workload-class sizing, guaranteed memory/headroom and queueing matter; changing the control plane cannot create RAM |
| Linux work should leave the Mac | The audio task's adjacent human instruction asks to move testing infrastructure off the Mac; the agent reports substantial memory freed and checks x86 capacity. | Placement preference should preserve Apple capacity automatically |
| Toolchain images drift | Xcode exists but its Metal compiler component is missing; it is installed in the disposable Mac guest. | Recipes need actual toolchain readiness checks and maintained versions under either platform |
| Shared dependencies are sometimes duplicated unnecessarily | A qualification setup creates separate voice/model runtimes; adjacent human feedback says the host/model runtimes exist for this purpose. | Return approved dependency bindings with the environment; allocating a VM alone is insufficient |
| Permanent agent crews and burst workers are both intended | An agent-orchestration task proposes a persistent controller and workers; human feedback asks for permanent crew plus temporary test workers across the lab. Its routing is described as configured backends, not discovery of spare machines. | Put one infrastructure allocation authority beneath agent/workload frameworks; do not let each maintain a conflicting physical-resource ledger |
| Access problems are distinct from hypervisor failure | A distributed-build task finds SSH rejected and model inference timing out, then checks the registered PVE API instead of assuming provisioning impossible. | Centralize provider access and report which capability is actually unavailable; do not equate a failed SSH path with a failed machine |

These examples cover useful successes as well as failures. Several search hits were merely injected skill rules, planning prose or unrelated application use of “cleanup”; they were not counted as observed infrastructure incidents.

## The missing service was already specified

The original home-lab discussion explicitly proposes fresh environments, queued capacity, reserved permanent services and automatic cleanup, and records that provisioner as unimplemented at that time.

More decisively, the October 19 [renewable test-environment plan](../plans/0001-renewable-test-environments.md) was explicitly proposed, with implementation and production promotion not performed at the inspected baseline. Its contract already includes task ownership, recipes, renewable leases, idempotent operations, checkpoints, recovery, grace, retention exceptions, protected headroom, scoped execution and verified cleanup. It permits active work to renew from minutes to weeks; it does not impose an arbitrary maximum active lifetime. The linked plan now incorporates this audit's additions.

The [current architecture](../architecture/current.md) describes procedural provisioning and no general implemented lease controller. That document is dated, so it was cross-checked with current registry capabilities and repository source layout; no clearly registered general allocator was discovered. This is not proof that no experimental allocator exists anywhere.

Recent home-lab timeline context also describes inventory reconciliation and streamed updates as a staged candidate. Reuse that work, but verify its exact promotion state before assuming it is deployed. Monitoring/observations and authoritative allocation remain different responsibilities.

## Compare the requested architectures against the evidence

### 1. Proxmox plus one allocator: preferred

This directly addresses the sampled failures while retaining already working Linux VM/LXC execution, cloud-init and guest administration. The allocator owns requests, resource reservations, task bindings, leases, dependency access and teardown. Native PVE and Apple executors carry out operations. Current registry/discovery and backup/access mechanisms remain shared infrastructure.

A Proxmox cluster can improve shared management, permissions and migration. It is **not required to start this service**, because independent hosts already expose APIs. Nor does creating a cluster supply a lab-wide task lease. Start where the value is: admission, ownership, Mac queueing and repeatable recipes. Decide cluster membership separately around stable wired connectivity, quorum, storage and actual migration needs.

Proxmox CRS currently documents placement/rebalancing of HA resources. Its resource pools are access-control groupings, not guaranteed RAM reservation pools. [HA/scheduler](https://github.com/proxmox/pve-docs/blob/master/ha-manager.adoc), [resource pools](https://github.com/proxmox/pve-docs/blob/master/pveum.adoc).

Risk: custom lifecycle machinery can become another hard-to-maintain platform. Bound the first slice, adopt maintained provider components and prove restart/timeout recovery. Do not turn a narrow environment service into a new hypervisor or duplicate the registry.

### 2. Cloudpods over Proxmox: conditional second

Cloudpods' real PVE integration is worth testing. It can connect existing endpoints and perform VM operations, so it may replace meaningful custom provisioning/catalog work. But it still needs the Apple backend and the lab-specific lease/artifact/retention policy. Independent external-provider image and placement boundaries must be exercised, rather than inferred from Cloudpods' native scheduler documentation. [PVE integration](https://www.cloudpods.org/en/docs/cmp/guides/cloudaccounts/cloudaccount/#create-proxmox-account), [provider source](https://github.com/yunionio/cloudmux/blob/master/pkg/multicloud/proxmox/host.go).

Choose it if a real trial shows less custom code and fewer operational steps for the same request contract. Reject a setup where Cloudpods becomes a second inventory/control stack while agents still manually choose hosts, repair images and coordinate Mac capacity.

### 3. Native Cloudpods rebuild: useful challenger, weaker evidence for migration now

Its creation-time scheduling policies are closer to a private-cloud resource request. Required/preferred/excluded host, network and storage tags are a real advantage. [Placement policies](https://www.cloudpods.org/en/docs/onpremise/guides/scheduler/schedtags/).

However, this audit does not show that rebuilding the Linux layer fixes the dominant incidents. It still needs Apple execution/consent management, dependency bindings, lease semantics and evidence policy. Native migration must additionally requalify existing LXC GPU and Quick Sync paths and persistent-service recovery. Its native stack introduces k3s/control-plane/database and compute services. [Native compute](https://www.cloudpods.org/en/docs/onpremise/getting-started/host/), [HA deployment](https://www.cloudpods.org/en/docs/onpremise/getting-started/ha-ce/).

Cloudpods should move to first place if the exact OSS deployment demonstrates safe admission and complete environment lifecycle with substantially less custom integration. That is a testable reason to migrate. A better initial-create abstraction alone is insufficient evidence of a better whole lab.

## Make the Mac a real shared worker

The Mac problem has three layers: capacity/ownership, execution recipes, and Apple consent/GUI identity. Neither PVE clustering nor either Cloudpods arrangement removes the last layer.

### One authority, multiple execution modes

The common allocator should offer at least:

- **Apple build/test guest:** macOS image with pinned Xcode, simulator runtimes, command-line tools and Metal component.
- **GUI-test guest:** same lifecycle with a verified console and stable signed automation runner.
- **Host/device session:** an explicit exclusive reservation when a real microphone/camera/USB device, host application or login session is essential. This is not equivalent to a disposable guest and must not be silently substituted for one.

A small authenticated host-side executor should own VM inventory and clone/start/stop/delete/console operations. Agents receive environment access and status, not repeated host setup procedures. Jobs queue with an owner, reason and renewal policy; they do not ask the operator whether another thread's guest may be disturbed.

Begin at the existing qualified concurrency limit, accounting for measured host capacity and protected services. Raise it only after measuring actual simultaneous workloads. Queueing improves usability even when only one guest can safely run. Move Linux builds/emulators and ordinary services away from scarce Apple capacity where qualified; reuse shared model/voice services through their supported interfaces.

### Reuse the existing signing solution

The security thread describes a valid-signature binary running in Terminal/user launchd but being rejected as a root service with OS_REASON_CODESIGNING. The original cause was still a hypothesis at that point. The later setup provides a stable code-signing identity, a service installation root and a scoped service helper. The reference deployment records that setup as installed; verify local equivalents before reuse.

Use that boundary instead of inventing a new unsigned binary or privileged execution route per task. The helper is not a general TCC bypass, and permission to install a service does not establish screen/microphone authorization.

### Reduce per-job GUI consent through prepared execution

Use versioned guest recipes with stable accounts and intended automation components; qualify a **fresh clone** for SSH, actual console capture/input and the requested test. Check permissions by executing the operation, not merely reading a stored flag. Toolchain or identity changes can invalidate assumptions, so promote image updates only after repeat validation.

Prefer the already qualified host-provided virtual console for viewing/input to a guest where it avoids giving every newly spawned agent screen-capture permissions on the physical Mac. Verify the exact runtime's console and networking path; no claim is made that this covers guest microphone or every app permission.

An audio task reports unauthorized microphone access and a later black/unreachable guest after virtual-audio recording; a kernel panic was only an agent hypothesis. Later context limits testing of granted microphone access because the VM has no microphone. The allocator must distinguish “macOS exists” from “actual audio capture qualified.”

For new app code whose purpose is to test consent, use a fresh permission state intentionally. For infrastructure automation, stable identities and prequalified recipes reduce repeated enrollment. Do not globally weaken the shared host or silently bypass real consent to make a test pass.

### MDM can help, with concrete limits

Apple's macOS 27 declarative AppSettings can suggest consolidated privacy defaults for supervised, AppKit-based apps, including Accessibility, camera, microphone and local network. The user still accepts a consent prompt. This is materially better than configuring every permission separately, but it is not a universal silent grant for arbitrary command-line tools. [Current Apple AppSettings documentation](https://developer.apple.com/documentation/devicemanagement/appsettings.md), [schema](https://github.com/apple/device-management/blob/release/declarative/declarations/configurations/app.settings.yaml).

The legacy PPPC ScreenCapture service cannot grant screen capture, only deny; other services have different policy support. [Apple PPPC schema](https://github.com/apple/device-management/blob/release/mdm/profiles/com.apple.TCC.configuration-profile-policy.yaml).

An OSS Apple-management component such as [NanoMDM](https://github.com/micromdm/nanomdm) is a candidate for a separate management evaluation, not proof that this lab is enrolled or that its exact declarative features work here. Enrollment, APNs/certificate lifecycle, supported OS/app identity and user consent still need qualification. Prioritize stable worker execution and templates first; add management where it measurably removes recurring setup.

### Runtime options

Keep the existing verified Tart images for the first allocation experiment. Orchard already provides Apple worker/controller APIs, label selection and consumable resources; it is a possible backend implementation, not another independent memory ledger. Current Orchard/Tart licenses are source-available FSL, so distinguish them from strict OSS. [Orchard resources](https://tart.run/orchard/using-orchard-cli/), [architecture](https://tart.run/orchard/architecture-and-security/), [license](https://github.com/openai/orchard/blob/main/LICENSE).

Lume is the alternative to qualify if strict OSS is required; its API can manage Apple Virtualization guests, but that alone does not prove the current Xcode/console/audio recipes migrate successfully. [Lume docs](https://cua.ai/docs/lume), [component licensing](https://github.com/trycua/cua/blob/main/legal/LICENSING.md).

## Network identity is an independent dependency

A private Wi-Fi address is normally useful for client privacy. If an interface's
identity changes while a DHCP reservation targets the earlier identity, the DHCP
server can assign a different address. Exact-source-IP access rules can then reject
a valid cryptographic key. The audit observed an address/access-rule mismatch;
the saved Wi-Fi rotation setting was inaccessible, so its exact mode was not verified.

Stabilize server identity where appropriate and resolve endpoints centrally. Fixed
private addressing on a trusted network can preserve privacy while matching a
reservation; DHCP remains appropriate. Presence updates cannot repair every
external exact-address ACL unless that binding is explicitly implemented.

## First useful delivery and decision gates

1. Implement the existing plan's smallest real-lab slice: one PVE Linux recipe and one Apple recipe behind a durable acquisition API, with active leases, pending queue and protected headroom. Use existing qualified Mac concurrency; no new cluster is needed to demonstrate value.
2. A fresh agent requests an environment without naming a host or guest ID. It gets ready access, actual capabilities, shared-service bindings, artifact location and an automatically renewable lease.
3. Two agents contend for the Mac. One runs; the other sees an actionable queue state. Restart the controller, resume ownership and protect the first guest. An unrelated cleanup request exposes the active owner and impact before any explicitly authorized override.
4. Exercise a guest requiring GUI automation and an Apple build on fresh clones without repetitive infrastructure-consent setup. Separately report real microphone/screen/device cases still requiring consent or a native session.
5. Expire/release an owned guest after acknowledged artifact export. Verify disk and credential deletion; renew another and prove it survives. Include indefinite human-review retention where explicitly requested.
6. Compare the same contract through Cloudpods' PVE provider, then native Cloudpods if its implementation promises a meaningful reduction in custom code. Qualify a real native worker only through an authorized migration/capacity plan; no nested or synthetic lab stand-ins.

Measure unattended completions, human interventions, acquisition/queue latency, image failures, lost environments and orphan allocations. Do not select a platform on feature count or a falsely precise weighted score. The audit supports this order; it does not authorize deployment or migration.
