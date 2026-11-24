# Current home-lab architecture

Reconciled 2026-11-23 against `main` at `63624fb`, the verified inventory and
backup deployments, and the integrated staging candidate on
`feature/remaining-lab-work`. The registry
holds current machine inventory, access references and operational evidence.

The [vision](../vision/home-lab.md) describes intended outcomes. The
[environment leasing plan](../plans/0001-renewable-test-environments.md) tracks
staging evidence and remaining delivery.

## System boundaries

The lab combines persistent household services, agent orchestration and development
workloads, shared application capabilities, and the infrastructure needed to operate
and recover them. It is a heterogeneous collection of managed hosts and guests,
not one uniform scheduler-controlled compute cluster.

```mermaid
flowchart TB
  People[People: home use / remote administration / development]
  Agents[BB and coding agents]
  Registry[Registry: discovery and operational records]
  Dashboard[Dashboard and browser access]
  Compute[Proxmox guests / native macOS / platform-specific workers]
  Home[Home automation and media]
  Shared[Speech / models / mail / hosted applications]
  Tests[Project integration and disposable staging]
  Router[Router: household LAN allocation]
  Recovery[SSH / hypervisor console / KVM / supported power management]
  Backup[Central configuration backup]
  Drive[Encrypted Google Drive backups]
  People --> Home
  People --> Agents
  People --> Dashboard
  Dashboard --> Registry
  Agents --> Registry
  Agents -->|Current provisioning via runbooks and administration| Compute
  Agents --> Shared
  Compute --> Home
  Compute --> Shared
  Compute --> Tests
  Compute -->|Presence and observations| Registry
  Router -->|DHCP and reservations| Compute
  Dashboard --> Recovery
  Recovery --> Compute
  Backup -->|Discover sources| Registry
  Backup -->|Collect configuration| Compute
  Backup --> Drive
```

Arrows show relationships, not comprehensive network flows. The router also serves
household devices and management appliances. External service access and third-party
cloud dependencies have their own authentication and availability requirements.

## Compute and workload isolation

Proxmox hosts provide full VMs and Linux containers. Full VMs provide independent
operating systems and the boundary used for service-development staging. Containers
host suitable persistent Linux applications with lower guest overhead and explicit
host-kernel/device dependencies. Neither choice is universal: guest type follows
vendor support, hardware access and the workload's isolation requirements.

The Apple platform supplies native macOS applications and platform-specific testing.
Native service managers and Docker-based deployments coexist with Proxmox workloads;
the entire lab has not been converted to one runtime. Platform tools and registered
templates are the provisioning mechanisms, not evidence that every requested OS,
emulator or hardware-accelerated combination is qualified.

GPU and media acceleration are host-specific capabilities. Guest assignment,
drivers, sharing constraints and workload qualification must be read from current
records. A host GPU metric is not proof that a particular guest can use that GPU.
Personal Windows/gaming use and dual-boot availability are also allocation constraints,
not capacity that an agent may interrupt automatically.

There is no documented unified capacity scheduler or universal shared-storage/HA
layer in this architecture. Local guest disks and physical-host dependencies matter.
A stopped guest can still consume storage. Do not infer migration, failover or
cross-platform portability from the presence of multiple hypervisors.

## Persistent household services

Home Assistant runs as HAOS in a persistent Proxmox VM. Its automation path includes
Matter service integration and a separately powered ESPHome Bluetooth proxy for
supported devices. Those transports are dependencies of device behavior; they are
not interchangeable with the Home Assistant application or a Mac Bluetooth process.

Plex runs as a persistent Linux container service, with a separate scheduled rclone
media-sync function in the same guest. The sync selects the configured Google Drive
media folders for local Plex use. Library metadata, media availability and hardware
transcoding have distinct validation needs. Media synchronization is separate from
the configuration backup system, even though both use Google Drive-related tooling.

Unattended startup is part of these deployments. Routine application operation does
not require a human console login. Their persistent configuration and device/media
relationships must survive migration and maintenance. Protected placement and safe
changes are currently operating procedures, not guarantees of an automated allocator.

## Agent orchestration and project workloads

BB provides persistent coding-agent orchestration; related development/integration
instances are distinct workloads. Its current placement and dependencies belong in
the registry. The architectural intent to use suitable persistent compute is not
proof of a completed orchestration migration.

Agents discover resources through the home-lab skill and registry, then use recorded
SSH or hypervisor access and templates to operate guests. Project services include
persistent integrations, reviewable application deployments and disposable staging.
These are separate from the agent process that requested them. A test VM need not
run the coding agent.

Stable provisioning uses registry runbooks and hypervisor administration. The
disposable allocator shares one lease contract across Linux and Tart. Staging
verified scoped Ollama access and NativeLink cache reads, writes and remote
execution against an isolated instance of the deployed worker. Release and
credential revocation both gated task access.

## Shared application capabilities

The shared speech service exposes speech-related capabilities with Pocket TTS and
voice selection; a named voice is a supported option within the service, not a
separate infrastructure stack. Native platform supervision is part of its recorded
deployment. Model inference is a separate service role, with its own capacity and
hardware requirements; speech and model inference should not be conflated.

Agent email uses a persistent mail server with protocol endpoints and a separate
webmail interface. Hosted applications and shared integrations have their own
service records and deployment lifecycles. Registry dependencies describe how to
find these services; credentials and caller authorization remain separate.

Tests may call authorized shared services through supported interfaces. That does
not authorize replacing a stable implementation with candidate code. Work requiring
a modified dependency follows the disposable-development isolation rules.

## Networking, identity and access

The household eero router is the IPv4 allocation authority. DHCP is preferred;
static LAN assignments require matching router reservations for the interface's own
MAC. Isolated guest/NAT networks, container networks, VPNs and loopback are different
address scopes. Their existence does not authorize claiming another household lease.

Enrolled metrics agents also report address presence, independently of logged-in
users. Registry bindings update selected service, monitoring and access endpoints
while SSH keys and TLS identities remain tied to machine identity. This is explicit
enrollment and binding, not automatic discovery of every device or allocation of IPs.

Human access combines the authenticated public dashboard, browser SSH through
Guacamole, and supported hypervisor, KVM and AMT interfaces. Some appliance paths
require LAN/VPN reachability. Network KVMs are independent devices with their own
credentials, addresses and certificates; reaching one does not establish working
video, host login or host power control. Recovery capability is verified per machine.

Public access uses authenticated HTTPS proxy/tunnel routes where configured. Not
all service endpoints are public. Private client configuration and protected secret
stores supply bootstrap and credentials. The current operator dashboard has powerful
access; it is not the scoped authority boundary proposed for ordinary test agents.

## Discovery, observability and administration

The registry holds machine and service identities, relationships, capabilities,
runbooks, access references, backup coverage and dated observations. Skills carry
reusable procedures. Public source carries implementations and examples. The
registry is authoritative operational information, not proof that an observation
is still fresh or a desired configuration has been applied.

The dashboard adds a graphical machine/guest/service map alongside separate
machine, service and backup views, provides browser access,
and displays low-overhead CPU/RAM/GPU/disk history. Collectors retain bounded local
history and stream while viewed. Collectors also serve local guest inventory over
mutually authenticated TLS; the dashboard receives an initial cached report,
then changes and freshness heartbeats, and expires stale states. Collector-side
hypervisor sampling continues at its configured interval. **Reconcile now**
reloads the catalog and reconnects to the collector streams. Startup also
loads current records. Background runtime collection uses mTLS and browser
terminals use pinned SSH. Health checks, guest inventory and address presence
remain separate from resource streaming; some run independently of viewers. Cached
metrics history is not a durable experiment archive.

An operator-triggered IP audit runs from an operator machine and compares router
authority with actual configuration. The dashboard displays the latest published
result and holds no audit credentials. The audit reports violations separately
from unverified coverage. It is read-only detection,
not preventive admission control. Routine monitoring is deterministic code, not
scheduled agent inference. Warnings and incomplete verification remain visible work.

The [registry/dashboard architecture](registry-dashboard.md) documents the HTTPS
API, SQLite state, collectors, authentication, update cadence and source modules.
[Plan 0005](../plans/0005-inventory-coverage-and-change-delivery.md) tracks metadata
change delivery, native event sources and coverage beyond hypervisor guests.

## Allocator staging

One durable SQLite ledger governs a fixed Debian slot, an on-demand Proxmox
pool, a dedicated CUDA slot and a prepared Tart Xcode slot with managed Metal.
Three Proxmox nodes ran independent leases concurrently and refused a fourth
request at capacity. Capacity checks use live provider observations. Task
credentials bind a principal to its lease and grant only the selected service
dependencies. The Debian recipe initializes the canonical workspace before
readiness. The SCSI/ext4 recipe supports qualified active disk growth with
durable intent and replay after controller restart.

The controller records intent before provider mutation. Exact guest and
storage identity, provider task history and an independent ledger witness
fence cleanup and replacement. An operator can reconcile explicitly proved
additional provider activity; normal release still requires encrypted
checkpoint readback before capacity returns. A cross-host drill fenced the
former controller before adopting a live lease. Missing and stale witnesses
were rejected until the current independent witness was restored. Recovery
from an older backup and loss of all independent witnesses still need a
qualified procedure.

Codex and Pi resumed real interrupted tasks and restored verified work.
The Pi trial measured checkpoint, export and replacement time. Owner alerts
passed retry, deduplication and exact routing to a controlled inbox.
Standing credential enrollment, external alert delivery and sustained
periodic checkpoint behavior remain acceptance items.

Managed Tart completed simulator and Metal work. A separate clean Tart
install restored the exact macOS base, installed the matching Apple-signed
Xcode app and simulator runtime, and passed Swift, Metal and simulator input.
Native host admission and cold recovery have their own qualification.
The operator deferred Windows and physical-device qualification.

[Ledger](../../allocator/rust/src/ledger.rs) · [Proxmox placement](../../allocator/rust/src/proxmox_dynamic.rs) · [Tart worker](../../allocator/rust/src/tart_worker.rs) · [Export](../../allocator/rust/src/export.rs)

[Allocator acceptance](../plans/0001-renewable-test-environments.md)

## Production runtime

The React dashboard and Rust dashboard and registry services run on the existing
production host. The rollout retained the catalog database, TLS credentials,
public sign-in route and collector identities. Authenticated reconciliation
completed for all six configured collectors and the live inventory stream
delivered events. Five eligible collectors now report native hardware and
delivery mode. Backrest uses the Rust collector for 18 fleet plans, with fresh
encrypted registry and host snapshots and registry archive verification.
The production allocator runs alongside the registry and dashboard. Its
enrolled provider, capture and recovery roles have production credentials.
The registry records the artifact identity, cutover and recovery evidence.

## Registry platform releases

The registry, dashboard, allocator and local lease workers share one
checksummed release directory. In staging and production, the units start
under `lab-platform.target` and resolve their executables and assets through the
same release pointer. A failed authenticated check restores the previous unit
definitions and release pointer. The catalog database and independently
witnessed lease ledger keep their own state, recovery and authorization.

The [deployment procedure](../../deploy/README.md) uses the same release in
staging and production. The live registry records the release ID and where it
has been exercised.

## Configuration backup and recovery

One central Backrest controller orchestrates configuration exports using registry
source resolution and constrained, pinned SSH access. Restic and rclone provide
versioned encrypted storage in Google Drive, the sole retained backup destination.
Temporary export staging and working caches are local working state. A
failed collection can leave staging that must be inspected before retry.
Private manifests and restore evidence determine actual coverage. The scoped
collector in [PR #1](https://github.com/csells/home-lab-registry/pull/1) shipped
with verified encrypted round trips for 17 remote collection paths. Source
details are in the [registry/dashboard architecture](registry-dashboard.md#configuration-backup-and-recovery).

Configuration backup, media synchronization, source control and task artifacts serve
different purposes. Git preserves public source; it does not replace private
configuration recovery. Media files are not implied to be in configuration archives.
The staged allocator retains task checkpoints in a separate encrypted repository
at the same destination. Configuration archives cover the declared configuration
scope, and task checkpoints cover the declared workspace.

Registry recovery requires exported catalog data and private bootstrap/access
material retrievable without the failed registry host. Recovering a service also
requires its runtime dependencies and a viable destination. Archive validation,
application restoration and whole-lab recovery are different levels of evidence.
There is no implied automatic host failover or zero-data-loss disaster recovery.

## Change, failure and verification boundaries

Service source development runs in an isolated disposable VM. Stable promotion
requires approval for the candidate, production verification and preservation of
recoverable rollback artifacts. Dedicated staging is then deleted and records retired.
Third-party installations and migrations follow their authorized backup/cutover
procedures rather than automatically acquiring a service-development staging gate.

Host failure can remove all guests and local state on that host. Registry or access
service failure can impair discovery and administration even while household services
continue. Router, public access and backup-provider failures affect different paths.
Independent console access and recovery material reduce those dependencies; they do
not remove the need to exercise recovery.

This review read catalog deployment descriptions and implementation source, not
all live application flows. Current status, restore results, hardware readiness and
remaining exceptions must be checked in the registry and verified before operations.
When a plan is implemented, update the affected architecture boundaries with actual
behavior and validation; keep proposed capabilities in plans until then.
