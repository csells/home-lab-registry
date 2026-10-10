# Proxmox service deployment and Community Scripts

Research checked 2026-10-06. These are reusable deployment patterns, not a service catalog. Discover actual placements, versions, dependencies and runbooks through the registry. Recheck upstream documentation and installed-version CLI help before executing commands; upstream documentation can describe features newer than the installed release.

## Choose the maintained path first

For an existing third-party service, read its vendor installation/migration guidance and look for a maintained [Proxmox Community Scripts entry](https://community-scripts.org/scripts). Prefer the matching maintained helper over manually reconstructing its installation, dependencies, hardware setup and updater. Inspect current support status, known issues and required privileges. Preserve the helper's supported OS and defaults unless the workload requires an override; document the reason and upstream update compatibility.

Manual installation is a fallback for a concrete missing capability, unsupported host/version, demonstrated helper defect or explicit user constraint. Record the evidence in the deployment runbook rather than treating familiarity with shell commands as a reason to bypass maintained tooling. Use vendor instructions for the fallback; avoid creating a parallel installer/updater unnecessarily.

Community Scripts is independently maintained, not built-in Proxmox application support or a vendor support guarantee. Its contribution process and shared implementation provide accumulated fixes and review. They do not certify every hardware combination, migration or application workload. Check the [support policy](https://github.com/community-scripts/ProxmoxVE/blob/main/SECURITY.md) and [contribution process](https://github.com/community-scripts/ProxmoxVE/blob/main/CONTRIBUTING.md); do not substitute popularity or a completed installer for destination validation.

## Select the guest type deliberately

| Workload pattern | Lab default | Reason and boundary |
| --- | --- | --- |
| Plex with a shared Intel GPU | Community Plex helper, unprivileged LXC | Low overhead and host-driver GPU access. Validate actual hardware transcoding. |
| Home Assistant with Supervisor and managed apps/add-ons | Community HAOS VM helper using the official HAOS image | Preserve the appliance's supported operating/update model. HA Container is a different installation type. |
| Application distributed primarily as Docker Compose | Vendor Compose stack inside a Linux VM | Keep the application's maintained packaging; Proxmox manages the VM, while Docker manages its containers. |
| Windows, a distinct guest kernel, or stronger isolation | Full VM | LXC shares the host Linux kernel and cannot substitute for these requirements. |
| Agent-developed service candidate | Disposable VM under the global development rule | Keep candidate code isolated; reuse existing shared services under the [testing exception](../SKILL.md#use-shared-production-services-for-testing). Review/promotion requirements apply to development, not ordinary third-party migration. |

Explain VM versus LXC when that distinction changes isolation, resource use or device ownership. A casual reference to a “VM” is not sufficient reason to silently replace a working container plan. Honor an explicit technical guest-type requirement. Use the application's supported operating model rather than forcing every service into the same guest type.

Proxmox manages both KVM VMs and LXC containers; community app helpers configure those primitives. Current upstream also documents OCI-to-LXC application containers as a technology preview. Do not confuse that feature with Docker Compose orchestration or built-in Plex/HAOS lifecycle management; prefer an established deployment path for persistent household services. [Proxmox container model](https://github.com/proxmox/pve-docs/blob/master/pct.adoc)

## Inspect and execute a helper without forking its maintenance

1. Verify the target Proxmox host, free guest ID, capacity and management recovery. Read the helper's current page, entry script, application installer and relevant shared functions. Note its OS, privilege/GPU defaults, downloaded package/image sources, network changes, telemetry options and update behavior.
2. Use canonical project sources. Helpers run privileged code on the host and can fetch additional code. Fetching one entry script at a commit does **not** pin its dependencies: the application scripts and shared engine are separate repositories, each normally tracking `main`.
3. For a reproducible deployment, select compatible reviewed revisions of both repositories and set their documented source roots. Record the exact entry URL/ref, both revisions, overrides and installed package/image versions in the runbook. Review the full fetch chain before claiming it is pinned. Package repositories and appliance release downloads can still change independently.
4. Run the upstream helper in its documented environment, choosing supported options for this host. Capture nonsecret progress and errors. Inspect resulting `pct config` or `qm config`; do not stop at the success banner.
5. Apply only the needed service-specific configuration and migration steps. Keep extension files separate from helper-managed files where practical. Preserve the maintained update route and check compatibility before each upgrade. Report reproducible upstream defects through the project's normal process only when communication is authorized.

The documented selectors are `COMMUNITY_SCRIPTS_URL` / `COMMUNITY_SCRIPTS_ROOT` for application scripts and `COMMUNITY_SCRIPTS_CORE_URL` / `COMMUNITY_SCRIPTS_CORE_DIR` for the engine. Check the current [loading/resolution documentation](https://github.com/community-scripts/core/blob/main/docs/loading.md) before using them. Pinning improves reproducibility; reviewing and advancing pins is necessary to receive later fixes.

## Plex recipe

Read the [Plex helper](https://github.com/community-scripts/ProxmoxVE/blob/main/ct/plex.sh) and [application installer](https://github.com/community-scripts/ProxmoxVE/blob/main/install/plex-install.sh). At research time the helper selects Ubuntu 24.04, an unprivileged LXC, GPU setup, 2 CPU cores, 2 GiB RAM and 8 GiB root disk. Treat these as installation defaults, not a sizing promise for migrated metadata, transcode scratch or media. Use the helper's supported OS rather than substituting a newer Debian release without cause.

The installer uses Plex's package repository and its shared hardware-acceleration setup. The current updater uses APT and handles old Plex repository definitions. Use that path rather than adding another Plex updater. Provision media separately from root/configuration when useful; make backup inclusion explicit.

For LXC, inspect render nodes, mapped ownership and Plex group access. Keep the container unprivileged and AppArmor enabled; do not fix access by globally opening device permissions or switching to a privileged container without a demonstrated requirement. Use the helper's hardware setup first and installed `pct` device mapping support for a justified correction. This uses the host GPU driver; VFIO blacklisting and PCI passthrough are different mechanisms. [Device mapping reference](https://github.com/proxmox/pve-docs/blob/master/generated/pct.conf.5-opts.adoc)

Whole-GPU passthrough to a VM removes that device from host use. Verify IOMMU grouping and the effect on local/KVM/AMT video before changing bindings; preserve independent power recovery. Do not trade away recovery accidentally to satisfy a terminology misunderstanding. [PCI passthrough](https://github.com/proxmox/pve-docs/blob/master/qm-pci-passthrough.adoc)

Validate hardware decode **and** encode in a real forced transcode, including relevant HDR tone mapping, audio and subtitle behavior. A render node, `vainfo`, direct playback or installer success alone is insufficient. Check Plex Pass/account requirements and platform limits against [Plex hardware streaming guidance](https://support.plex.tv/articles/115002178853-using-hardware-accelerated-streaming/) and [HDR guidance](https://support.plex.tv/articles/hdr-to-sdr-tone-mapping/).

The helper installs Plex; it does not migrate an existing server. Follow [Plex migration guidance](https://support.plex.tv/articles/201370363-move-an-install-to-another-system/): preserve consistent data/settings, validate destination ownership and media paths, and avoid automatic trash emptying while paths change. Plex explicitly says cross-OS migration is outside that article's official support. For macOS-to-Linux, explicitly verify identity, translated preferences, watch state, library matches, account sharing and remote access rather than promising all preferences transfer automatically. Keep the source recoverable and prevent two live copies of the same identity.

## Home Assistant recipe

Use the [HAOS VM helper](https://github.com/community-scripts/ProxmoxVE/blob/main/vm/haos-vm.sh), which imports the official HAOS QCOW2 appliance. At research time it defaults to stable, 2 vCPUs, 4 GiB RAM and 32 GiB disk. The vendor's VM minimum is 2 vCPUs/2 GiB; size for actual integrations and available capacity, not just that minimum. [Official VM installation](https://www.home-assistant.io/installation/alternative/)

HAOS includes Supervisor and managed apps/add-ons; Home Assistant Container does not provide the same appliance management. Do not choose the similarly named container helper when HAOS is intended. [Installation-type distinction](https://www.home-assistant.io/faq/ha-vs-hassio/)

Restore the application's backup and verify integrations, automations, radio/USB or network-device access and required discovery paths. Validate Matter/Thread requirements when those integrations are in scope. Avoid two restored controllers operating live devices. Update HAOS through its supported application/OS controls; provisioning helpers do not replace HAOS's lifecycle management.

## Operate the host and guests

Keep application services and build runtimes in guests. Reserve host and persistent-service resources before admitting disposable work; read each machine's headroom policy from its registry record rather than assuming a universal Proxmox minimum. Recheck configured guest RAM as well as available RAM, disk allocation and real consumption. Keep explicit free-space headroom and bound temporary transcode/download usage.

Use stable host addressing and deliberate guest addressing. Record a missing DHCP reservation as a gap; assigning a currently leased address statically does not reserve it in the router. Protect administration with LAN/private authenticated access and least-privilege credentials. Finalize identity/addressing before considering clustering. A cluster adds quorum obligations; it does not create backups or make a GPU-bound service freely migratable. [Cluster guidance](https://github.com/proxmox/pve-docs/blob/master/pvecm.adoc)

Use repositories matching the installed PVE/Debian release. Proxmox recommends enterprise for production; no-subscription is a lab tradeoff with less validation, not an equivalent production channel. Do not enable test repositories or apply a generic host “post-install” script wholesale as an incidental service install. Review release/upgrade notes, back up and schedule reboots. [Repository guidance](https://github.com/proxmox/pve-docs/blob/master/pve-package-repos.adoc)

Enable and test both guest onboot/startup ordering and the application's system services/timers. A login must not be required. After a host reboot, verify authenticated host management, guest startup, hardware access and real service endpoints. A scheduler definition or enabled flag alone is not that test.

## Backups, restoration and handoff

Use Proxmox's managed export facilities where suitable, with the registered central service retaining backups only in Google Drive. Do not add a Proxmox Backup Server or another retained backup destination without new user direction. Follow the [lab backup SOP](backups.md) for encryption, Drive placement, retention and restore evidence. [Proxmox backup guidance](https://github.com/proxmox/pve-docs/blob/master/vzdump.adoc)

Inspect every root disk and mount. Storage-backed extra LXC mounts require explicit backup inclusion; `backup=0` excludes them. Bind/device mount contents are not saved by `vzdump`. A restored mount declaration does not restore those files. Do not promise CT snapshot-mode backups on storage lacking snapshot support, or treat local snapshots as off-machine backups. Record how excluded media is recovered and test the configuration/database restore. [LXC backup semantics](https://github.com/proxmox/pve-docs/blob/master/pct.adoc#backup-and-restore)

Restore drills need isolated network/device access **and separate scratch/cache paths** so a restored application cannot disrupt the original. Capture evidence outside disposable guests before cleanup. Keep installers, app-native exports and hypervisor backups in their respective roles; a helper never replaces recovery testing.

Before declaring migration complete, verify the actual client flow, settings/data preservation, unattended reboot, scheduled health and off-host backup. Update the registry's deployment, dependencies, probes, backup fields and runbook. Record helper provenance and justified deviations there; do not add a hardcoded service list to this skill. Retire an earlier candidate only after its data/evidence is accounted for and identity/ID are reverified.
