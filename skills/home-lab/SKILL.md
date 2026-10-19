---
name: home-lab
description: >-
  Operate Chris's mini-PC home lab: discover machines and KVMs, install or maintain Proxmox hosts, manage persistent services, and provision Windows/Linux/macOS/Android/iOS test environments for agents. Use for this lab's inventory, remote access, capacity, deployments, and environment lifecycle.
---

# Home lab

The central [registry](references/service-registry.md) is the sole source of machine and service information: identity, hardware, installed software, access, recovery, templates, placement, dependencies, reservations and health. Query it before selecting or operating a machine or service. This skill contains discovery instructions and reusable procedures only. Adding, changing or retiring machines or services must not require editing the skill.

Read the selected record's runbooks, constraints and dated evidence, then verify live identity and capacity before changes. Conversational names need not be DNS names; a KVM address identifies the console appliance, not the attached computer. Missing or stale information must be investigated and updated in the registry, never filled in from a duplicate inventory here.

Use the Dashlane skill for vault credentials and browser-use for KVM console operations. Keep secret values out of the registry, skill, screenshots, command output and version control. Store only credential references and authenticated access instructions in registry records.

## Proxmox service provisioning

Before installing or migrating a third-party service, read [Proxmox deployment practices](references/proxmox-deployments.md). Prefer a maintained Proxmox Community Scripts recipe that fits the vendor's supported installation model over rebuilding its installer, hardware setup and updater. Inspect current sources/support, use supported defaults, document necessary overrides, and verify the destination. Manual assembly needs a concrete reason.

Choose the guest type by workload: normally the community Plex LXC recipe for shared Intel graphics, the HAOS VM recipe for Home Assistant with Supervisor/apps, and a Linux VM for vendor Docker Compose stacks. Explain any consequential VM/LXC distinction before switching architectures; do not interpret casual terminology as a new requirement. These are deployment patterns, not a catalog or proof of installed services.

Apply the global disposable-VM development rule to service development/source changes, with the [shared-service testing exception](#use-shared-production-services-for-testing) below. An authorized third-party installation or migration follows its backup, validation and rollback plan; it does not acquire a staged-code promotion gate merely because it needs operational scripts. Preserve explicit user constraints and approval requirements.

## Operating model

Agent orchestration runs on suitably provisioned persistent infrastructure. Agents request disposable test environments on workers selected by architecture, hardware access, capacity and reservations; the test guests need not host coding agents. Discover existing orchestration and provisioning capabilities through the registry before implementing another.

Select compatible x86 workers for Windows/Linux guests and Apple hardware for macOS VMs or Xcode/iOS Simulator. Discover each machine's permitted roles and installed capabilities from its registry record. Place general persistent services on suitable Proxmox guests while preserving settings and data. Read current placement, migration constraints and dependencies from each registry record; the target architecture is not evidence that migration has occurred. Android needs a verified hardware-accelerated worker; treat nested virtualization as unproven until exercised. A simulator does not replace physical-device testing.

Provide disposable application test environments cloned from maintained templates, with resource limits, ownership, expiry, and external artifact storage. Reserve resources and separate permissions for persistent household/production services. Queue work when capacity is exhausted; do not oversubscribe worker memory.

For existing host maintenance or environment requests, read [operations](references/operations.md). Distinguish implemented capabilities from the proposed provisioner: a hypervisor API alone is not a scheduler or lease manager.

## Use shared production services for testing

Use the lab's existing production services for tests through their supported interfaces. Chris explicitly authorizes this use: shared host, model, voice and agent runtimes are provided for this purpose. Prefer these services over provisioning duplicate runtimes or requesting new API keys. Do not ask for permission again merely because a service is labeled production or runs on a stable host.

This is an explicit exception to the global restriction on using stable runtimes as test backends: a disposable candidate may call an existing shared service and use its runtime. Keep candidate application code, its own runtime, workspace and state in the disposable VM when the development isolation rule applies. Calling a shared service does not authorize replacing its implementation, deploying candidate code into its stable installation, or changing unrelated production configuration or user data. Develop changes to the shared service itself in a separate disposable VM and follow the existing staged-promotion rule.

Discover endpoints, supported authentication, capacity and reservations through the registry. Follow the service's lease or concurrency protocol, keep test work identifiable, export evidence, remove test credentials from disposable guests, and release reservations when finished. Leave shared services running. Provision a separate dependency only when the test needs a modified version or the shared service cannot support the test safely or within available capacity.

## Promotion includes staging cleanup

Approval to promote a staged service to stable also authorizes and requires cleanup of that service's disposable staging environment, without a separate confirmation. After verifying the promoted production service end to end and preserving needed source, evidence and recoverable rollback/configuration backups outside staging, stop and permanently delete the staging service, its dedicated VM or container and disks, temporary credentials, relays, shares and routes. Release reservations, retire registry records and verify that allocated resources were returned to the pool. A stopped VM still consumes disk; stopping alone is not completed cleanup.

Recheck the exact host, guest identity and task ownership before deletion. Preserve shared dependencies, templates, unrelated environments and user data. If a concrete dependency or an explicit retention instruction prevents cleanup, record the reason, owner and expiry in the registry and finish all independent cleanup. Do not retain staging merely as an extra rollback copy when the required recovery artifacts are already preserved.

## Service login details on dashboard cards

When installing, registering, migrating or changing access to a service, maintain its dashboard card's login details as part of completion. Show the appropriate human username and provide the existing protected **Copy password** control when a reusable password applies, following the established Home Assistant/webmail pattern. Handle SSO, passwordless and API-only access explicitly. Read [service credential publication](references/service-registry.md#service-credential-publication) for metadata, private storage, rotation and live verification. Apply this to services in the current task; a skill update alone does not imply the existing fleet has been reconciled.

## Service configuration backups

For persistent-service deployment, migration, configuration changes or retirement, follow the [backup SOP](references/backups.md). Google Drive is the sole retained backup destination. Use the registered central backup service for scheduling, encrypted versioned storage and restore verification; do not create independent machine timers or another retained backup destination. Discover destinations and record actual coverage through the registry; a backup policy is not evidence that a service has a working backup.

## Proactive fleet health: warnings and errors

Own the health of the physical hosts, hypervisors, VMs, containers and services you operate. Watch for warnings as well as errors during provisioning, migrations, validation and ongoing maintenance. Inspect command output, service logs, scheduled-job results and health reports; a successful exit code or working service does not clear a warning. Include warning detection and follow-up in recurring monitoring, not just failure detection.

Investigate each warning for impact, urgency and the vendor's recommended remedy. Act ahead of deprecation, retirement and expiry deadlines, capacity exhaustion and hardware degradation. Check dates against the current date; do not defer a warning because the affected feature still works today. Follow through on fixes within existing authorization, preserve backups and rollback, and verify the remedy in the running system. Do not merely mention the warning and move on, suppress it, or wait for Chris to notice or for an outage to occur.

Record actionable warnings in the affected registry record with evidence, affected components, deadline when known, remediation, owner and next check. If blocked by credentials, consent or a consequential user decision, request the specific missing input promptly and continue independent work. Any deferral needs a concrete reason and follow-up date before the risk materializes; a confirmed benign warning needs its rationale recorded. Report unresolved risks separately from current availability, and close them only after verification. Keep this live tracking in the registry, never in the skill.

## LAN IPv4 authority

Eero is the sole authority for home-LAN IPv4 allocation. Prefer DHCP, with an eero reservation when a stable endpoint is needed. Before assigning any static address (including a temporary address or alias), create or verify an eero reservation for that exact address and the interface's own MAC, then read it back. Never choose an unreserved static address, even for installation, recovery or staging. A remembered lease, a reachable address or an eero device's seen-address list is not a reservation. Never convert a DHCP lease to static without verifying its reservation first. Apply this to hosts, VMs, LXCs, aliases, KVMs and AMT; distinguish isolated NAT, Docker, VPN and loopback networks from the household LAN.

Before provisioning or changing networking, read live eero devices/reservations using the eero skill, check for another owner, and verify an independent console recovery path. Read back every reservation change. Inspect persistent and live configuration, including pending Proxmox network files; update consumers, host-key aliases and TLS identities when an endpoint moves. Verify after a network reload or reboot without disrupting unrelated work. Never reclaim another device's lease or alter that device to accommodate a lab host.

Run this comparison on demand and after network changes or OS switches. Use deterministic code for address checks; do not schedule recurring inference/agent sessions to perform them unless Chris explicitly requests that expense. A dashboard audit must be read-only, show its observation times and coverage, and distinguish violations from incomplete verification. Do not label cached inventory as a fresh live audit. Inspect actual guest configuration as well as hypervisor metadata. Record each machine's MAC, all LAN addresses, assignment method, reservation name, timestamp, verification scope and evidence in the registry; read the records back. Warn on unreserved static addresses, duplicate claims, stale reservations and incomplete coverage. Keep audit schedules, outcomes and exceptions in the registry, not this skill.

For running systems, compare live addresses with persistent profiles and pending network configuration. For stopped guests and templates, inspect saved profiles read-only without booting them or writing their disks: use read-only block access and prevent filesystem journal replay, or parse the filesystem read-only; detach everything afterward. Inspect cloud-init inputs and saved guest profiles together. Deduplicate image tags and digests that refer to the same disk. Record `offline-config-verified` separately from live verification; recheck after boot or configuration changes. Reuse earlier offline evidence only after checking that disk identity and relevant configuration have not changed. An inaccessible active system or unreadable configuration remains unverified. For firmware appliances, use device settings or evidence tying the installed build to its source configuration and provisioning history; state any inference explicitly.

Resolve active conflicts first and retain incident evidence. Before removing an address, identify why it exists and every service or consumer that depends on it. After the persistent correction, verify through a network reload or authorized reboot, test affected services at their real endpoints, and check the disputed address from another LAN host for at least two minutes with one-second ping/ARP observations. The offending MAC must never appear. Confirm the host no longer holds the address and eero's device record has aged out the old claim; report stale eero observations as pending rather than overriding contradictory evidence. Preserve the legitimate device's configuration.

## Changes and verification

Apply the current conversation's authorization to its identified machines. An authorization to repave one host does not authorize erasing the other hosts or migrating live services. Preserve an already-authorized action across turns rather than asking again.

Before boot, graphics, network or hypervisor changes, verify an OS-independent console and power-recovery path on the actual host. Exercise remote power-on after a clean shutdown and confirm authenticated administration returns; an enabled firmware option or reachable KVM login is insufficient. After host setup, verify identity, CPU/RAM/storage, wired networking, virtualization, authenticated remote management, and a reboot. Exercise an actual guest before claiming VM provisioning works. Verify a service through its real endpoint before calling a migration complete. Report incomplete checks explicitly.

After a reboot or OS switch, verify the intended OS through authenticated administration, its DHCP lease or matching reservation, persistent networking, failed units, required hardware, and automatic health registration. Exercise the metrics stream from the registry host with its normal authentication and certificate checks; an active daemon alone does not prove dashboard delivery. Check guest states and capacity, preserving unrelated workloads and stopped guests unless starting them is authorized. Record the actual boot route and any recovery limitation in the registry.

After machine or service deployment, migration, configuration changes or retirement, update its central registry record and verify the readback. Record actual setup, versions, addresses, access references, templates, dependencies, recovery and backup procedures, dated validation and remaining gaps there. Verify the real endpoint and recurring health publisher. Retire replaced components with history retained. Keep observed facts separate from intended roles; a reachable port is not functional validation or available capacity. Never copy machine or service inventory into this skill or its references.
