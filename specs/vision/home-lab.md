# Home-lab vision

Status: consolidated direction, 2026-11-20. This is the intended experience, not a claim that every capability is deployed. [Provenance](../research/source-provenance.md).

## Purpose

Turn the computers we own into a dependable platform for running the home, building software and exploring ideas. The lab should return more useful work and learning than it consumes in administration. Experimentation and migration are welcome when they produce a better system.

## A dependable home and room to experiment

Home automation, media, mail and other everyday services retain protected resources, durable data, known dependencies and recoverable backups. Development and temporary tests use isolated environments whose failures and cleanup preserve those services.

Permanent agent orchestration and working crews can coexist with burst workers, disposable application tests and review environments. The environment running a test need not host the coding agent that requested it.

## Ask for capability, receive a ready environment

An agent derives requirements from the user's task and asks one discoverable service for a suitable environment. It receives ready access and useful status without manually selecting a physical machine, guest ID, address, template or cleanup recipe.

The pool respects the qualities that make machines useful: operating system and architecture, CPU/RAM/storage, GPU access mode, Apple toolchains and simulators, physical devices, network placement, personal use and recovery constraints. A label such as GPU or macOS is useful only when the requested workload is qualified to use it.

Support Bazel builds, GPU workloads and testing across Linux, Windows, macOS, iOS
and Android while preserving personal desktop and gaming use.

Capacity is admitted safely or work queues with an understandable reason. Household headroom, existing workloads and specialist capacity are protected. Ordinary Linux work moves to appropriate Linux workers so Apple hardware remains available for Apple-specific work.

## Shared Apple resources without repeated setup

The Mac is a managed worker with authoritative host-side state, ownership, queueing and prepared execution environments. Routine jobs should reuse qualified images and stable signed automation components instead of repeatedly asking the operator to resolve infrastructure dialogs.

Apple builds, GUI tests and native host/device sessions have different capabilities and isolation. A simulator or VM does not imply physical microphone, camera, USB or device access. Consent and GUI-session requirements remain visible and are prepared deliberately. Application tests of permission behavior use the appropriate fresh state; infrastructure automation keeps a consistent identity.

## Work lasts as long as it needs

Environments belong to ongoing tasks and can be reused and renewed autonomously for minutes or weeks. Active work has no arbitrary lifetime cap. Human-review holds and explicit keep-until-further-notice instructions remain meaningful.

Ownership is visible across threads. A paused conversation is not proof of abandonment. Cleanup exposes affected consumers and preserves the data and evidence promised by the admitted policy. If an environment is lost, the agent can reconnect or reconstruct suitable work from a maintained recipe and acknowledged checkpoint, with any gaps stated honestly.

## Shared capabilities, one clear view

Agents and applications discover and use existing speech, models, mail, build and other services through supported interfaces. Provisioning returns dependency bindings rather than needlessly duplicating shared runtimes.

People and agents share a current view of what exists, who owns it, what it can do, what capacity is committed, and how fresh the observations are. The live registry owns inventory and operational records. Deterministic reporting, events and reconciliation maintain that view; routine housekeeping does not require recurring inference sessions.

Surface actionable warnings before outages, separately from current availability,
with an owner and a next check rather than an unexplained permanent warning.

## Access, privacy and recovery

People can review work and operate the lab remotely through authenticated access. Configured shared sign-in handles human access, while scoped machine credentials support unattended agents and LAN administration. Recovery remains possible when a normal service, registry or host is unavailable.

Stable network identity, verified endpoint resolution and known host identities prevent a DHCP change from becoming an unrelated project's access failure. Source, runtime secrets, user data, configuration backups and task evidence each have a deliberate home. Source control does not replace secret storage or encrypted recovery backups.

## Platform choices earn their place

Use maintained open-source components where practical. A common request interface can have multiple native backends; heterogeneous hardware need not become one identical runtime. Evaluate source-available components explicitly when appropriate.

The current recommended direction is one allocator over Proxmox and Apple workers. Native clustering and Cloudpods are options to test against real outcomes, not commitments to defend. Choose migrations that reduce total custom integration and operational burden while preserving useful capabilities and data.

## Project ownership

Home Lab Registry owns the reusable platform: catalog and observations, reconciliation,
capability discovery, environment allocation/lifecycle, provider adapters, and shared
Apple-worker tooling. The allocation controller has a separate execution and
authorization boundary from catalog reads/writes, even when developed in this repository.
One durable reservation authority admits capacity; observed inventory is not itself a lease.

Deployment-specific policies, host assignments, account configuration and integration
values remain operator-controlled. Application repositories retain their source.
Live inventory, credentials, user data and recovery archives stay outside public Git.
The existing [architecture](../architecture/current.md) describes implemented boundaries;
the [environment plan](../plans/0001-renewable-test-environments.md) describes proposed work.

## Success

More unattended task completion, less repeated setup, faster access to suitable environments, trustworthy ownership, fewer lost workspaces and orphan disks, recoverable evidence, and dependable household services. Measure these outcomes against actual workloads rather than selecting a platform by its feature count.
