---
name: home-lab
description: >-
  Operate a mini-PC home lab: discover machines and KVMs, install or maintain Proxmox hosts, manage persistent services, and provision Windows/Linux/macOS/Android/iOS test environments for agents. Use for this lab's inventory, remote access, capacity, deployments, and environment lifecycle.
---

# Home lab

The central [registry](references/service-registry.md) is the sole source of machine and service information: identity, hardware, installed software, access, recovery, templates, placement, dependencies, reservations and health. Query it before selecting or operating a machine or service. This skill contains discovery instructions and reusable procedures only. Adding, changing or retiring machines or services must not require editing the skill.

Read the selected record's runbooks, constraints and dated evidence, then verify live identity and capacity before changes. Conversational names need not be DNS names; a KVM address identifies the console appliance, not the attached computer. Missing or stale information must be investigated and updated in the registry, never filled in from a duplicate inventory here.

Use the operator’s configured credential vault and browser tooling for console operations. Keep secret values out of the registry, skill, screenshots, command output and version control. Store only credential references and authenticated access instructions in registry records.

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

Use existing shared services only when the operator has authorized their use for testing. Keep candidate code, runtime and state in a disposable VM. Authorization to call a shared service does not authorize replacing it, changing unrelated configuration or promoting candidate code. Obtain explicit approval before production promotion; follow the operator's isolation rules.

Discover endpoints, supported authentication, capacity and reservations through the registry. Follow the service's lease or concurrency protocol, keep test work identifiable, export evidence, remove test credentials from disposable guests, and release reservations when finished. Leave shared services running. Provision a separate dependency only when the test needs a modified version or the shared service cannot support the test safely or within available capacity.

## Promotion includes staging cleanup

Approval to promote a staged service to stable also authorizes and requires cleanup of that service's disposable staging environment, without a separate confirmation. After verifying the promoted production service end to end and preserving needed source, evidence and recoverable rollback/configuration backups outside staging, stop and permanently delete the staging service, its dedicated VM or container and disks, temporary credentials, relays, shares and routes. Release reservations, retire registry records and verify that allocated resources were returned to the pool. A stopped VM still consumes disk; stopping alone is not completed cleanup.

Recheck the exact host, guest identity and task ownership before deletion. Preserve shared dependencies, templates, unrelated environments and user data. If a concrete dependency or an explicit retention instruction prevents cleanup, record the reason, owner and expiry in the registry and finish all independent cleanup. Do not retain staging merely as an extra rollback copy when the required recovery artifacts are already preserved.

## Service configuration backups

For persistent-service deployment, migration, configuration changes or retirement, follow the [backup SOP](references/backups.md). Google Drive is the sole retained backup destination. Use the registered central backup service for scheduling, encrypted versioned storage and restore verification; do not create independent machine timers or another retained backup destination. Discover destinations and record actual coverage through the registry; a backup policy is not evidence that a service has a working backup.

## Proactive fleet health: warnings and errors

Own the health of the physical hosts, hypervisors, VMs, containers and services you operate. Watch for warnings as well as errors during provisioning, migrations, validation and ongoing maintenance. Inspect command output, service logs, scheduled-job results and health reports; a successful exit code or working service does not clear a warning. Include warning detection and follow-up in recurring monitoring, not just failure detection.

Investigate each warning for impact, urgency and the vendor's recommended remedy. Act ahead of deprecation, retirement and expiry deadlines, capacity exhaustion and hardware degradation. Check dates against the current date; do not defer a warning because the affected feature still works today. Follow through on fixes within existing authorization, preserve backups and rollback, and verify the remedy in the running system. Do not merely mention the warning and move on, suppress it, or wait for the operator to notice or for an outage to occur.

Record actionable warnings in the affected registry record with evidence, affected components, deadline when known, remediation, owner and next check. If blocked by credentials, consent or a consequential user decision, request the specific missing input promptly and continue independent work. Any deferral needs a concrete reason and follow-up date before the risk materializes; a confirmed benign warning needs its rationale recorded. Report unresolved risks separately from current availability, and close them only after verification. Keep this live tracking in the registry, never in the skill.

## Changes and verification

Apply the current conversation's authorization to its identified machines. An authorization to repave one host does not authorize erasing the other hosts or migrating live services. Preserve an already-authorized action across turns rather than asking again.

Before boot, graphics, network or hypervisor changes, verify an OS-independent console and power-recovery path on the actual host. Exercise remote power-on after a clean shutdown and confirm authenticated administration returns; an enabled firmware option or reachable KVM login is insufficient. After host setup, verify identity, CPU/RAM/storage, wired networking, virtualization, authenticated remote management, and a reboot. Exercise an actual guest before claiming VM provisioning works. Verify a service through its real endpoint before calling a migration complete. Report incomplete checks explicitly.

After machine or service deployment, migration, configuration changes or retirement, update its central registry record and verify the readback. Record actual setup, versions, addresses, access references, templates, dependencies, recovery and backup procedures, dated validation and remaining gaps there. Verify the real endpoint and recurring health publisher. Retire replaced components with history retained. Keep observed facts separate from intended roles; a reachable port is not functional validation or available capacity. Never copy machine or service inventory into this skill or its references.
