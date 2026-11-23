# Lab operations

## Discover before acting

Follow [registry discovery](service-registry.md). Read the selected machine's hardware, installed OS, storage, templates, constraints, access references and recovery runbooks. Verify live identity and capacity. Discover services and reservations associated with that host before changing it. Do not infer installations from a machine's name or historical evidence.

Use pinned SSH or authenticated hypervisor management for routine administration. Retrieve credentials through the referenced protected store. Verify host keys through an independent trusted channel when onboarding or reinstalling; never disable verification to bypass a mismatch. Distinguish OS, management-controller and external KVM addresses. If discovery is needed, compare normalized MAC addresses, leases and the responding device identity.

## Console and recovery

Follow the machine record's specific power, console and boot procedures. Test independent recovery before boot, network or graphics changes. A reachable KVM UI does not prove working video, keyboard or host power control. KVM restart controls may reboot the appliance rather than the computer. Prefer graceful OS shutdown while administration works.

For authorized OS installation, use official images with verified checksums, confirm target disks, mount virtual media through the documented device workflow, and inspect the actual console. Capture current video rather than trusting a stale browser frame. Record tested recovery paths and incomplete checks in the registry.

## Proxmox baseline

Read [deployment practices](proxmox-deployments.md) before provisioning applications. Prefer the maintained community recipe that fits the vendor's supported model. Keep the hypervisor focused on host duties.

Verify virtualization, CPU/RAM, disk health and capacity, networking, hostname/time, repository compatibility, storage, authenticated administration and reboot recovery. Use DHCP, or first verify and read back an eero reservation for the exact static address and interface MAC. Temporary installations, aliases and staging have no exemption. Choose storage for the actual disk count and network; do not add distributed storage or clustering without a demonstrated requirement. Follow the [backup SOP](backups.md).

## Disposable test environments

Reuse existing shared production services for test dependencies under the [standing testing authorization](../SKILL.md#use-shared-production-services-for-testing). Discover their supported interfaces and reservations before sizing or cloning a guest; do not duplicate model, voice or agent runtimes merely to satisfy candidate isolation. Keep candidate code and state isolated as required, and leave shared dependencies running during cleanup.

Find services with `lab-registry-client list --capability CAPABILITY` and read the selected record. Use its supported endpoint, credential reference and runbook from any authorized agent runtime. For the lease pilot, find an active record with `lease-pilot` and use its `lease_api`, registered client and trust material within the recorded qualification scope.

Use the task's private session with `CLIENT --session SESSION capabilities`. With individual credential files, use `--record RECORD --registry-client REGISTRY_CLIENT --ca CA --token-file TASK_TOKEN`. Follow the service record's authentication procedure to obtain or refresh the scoped task grant. A registry reader credential only permits discovery. If the grant is missing, report which service and credential scope you need. Use the same task owner after interruption.

Derive requirements from the work and query `leases` if the deployment supports it. Reuse an eligible owned lease, or acquire one with an idempotency key and wait for verified access. Renew while needed. Checkpoint the declared workspace and record the acknowledged recovery point. Release through the allocator and verify its disposition. After interruption or reclamation, recover the saved checkpoint or use `resume` with a fresh SSH key to restore into a matching environment. Report the recovery point and any missing state. If capacity is exhausted, report the allocator's reason immediately.

### Manual provisioning

Use this path only for an authorized operation when a suitable allocator is unavailable.

Query machine records for validated templates and compatible hardware. Use a minimal maintained Linux cloud image for ordinary application tests; match the production OS when relevant. Size CPU, RAM and disk for the actual workload. Do not assume a small smoke-test template includes browsers, Docker, development tools or coding agents.

Before cloning, check live guest allocations, available memory, storage consumption and registered reservations. Honor recorded host headroom and persistent-service constraints. If no allocator exists, serialize provisioning and explicitly track ownership and expiry; a lease written in a description is not automatic cleanup.

Choose an unused guest ID on the verified host. Use that machine's registered template ID, clone mode, access user and initialization instructions. Wait for initialization and guest-agent readiness, retrieve the guest address from the guest/hypervisor, then exercise the requested workload. Export logs and artifacts outside the guest before deleting it. Recheck exact host, guest ID, name and ownership before teardown; never delete a template, persistent guest or shared dependency as job cleanup.

A provisioning API is not a scheduler. Discover whether admission control, queues, reservations, expiry cleanup and scoped credentials actually exist. Report missing capabilities instead of assuming them.

## Platform and migration validation

Select platform support from machine capabilities in the registry. Windows needs a suitable VM; macOS virtualization needs compatible Apple hardware. iOS Simulator and Android Emulator do not replace physical-device testing. Verify hardware acceleration and any nested-virtualization arrangement with the actual test.

For service migrations, follow the registered service runbook, user constraints and vendor guidance. Preserve settings and data, capture a recoverable encrypted backup in the approved Drive destination, validate destination integrations and keep rollback available. Prevent duplicate active server identities or home-automation controllers. Verify unattended startup and scheduled health/backup jobs, then update machine and service records. Do not treat the intended placement as evidence of completed migration.

## Primary references

- [Proxmox administration guide](https://pve.proxmox.com/pve-docs/pve-admin-guide.html)
- [Proxmox ISO downloads](https://enterprise.proxmox.com/iso/)
- [GL.iNet remote OS installation](https://docs.gl-inet.com/kvm/en/tutorials/how_to_install_os_remotely/)
- [Tart quick start](https://tart.run/quick-start/)
- [Android emulator acceleration](https://developer.android.com/studio/run/emulator-acceleration)
