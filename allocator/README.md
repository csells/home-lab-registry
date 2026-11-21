# Linux lease pilot

This controller is a disposable staging experiment with one pre-provisioned
Proxmox VM. The operator prepares a verified Debian 13 guest and a stopped,
clean disk snapshot. The controller receives a TLS-verified Proxmox token
scoped to that guest for status, guest-agent audit and file writes, power and
snapshot rollback. It cannot clone arbitrary guests or expand the pool.
Before starting the fixed slot the controller checks live Proxmox memory, guest
commitments, and storage. It preserves the configured host floors and protected
guests, including protected guests that are currently stopped. Prepare a non-admin
guest account whose effective SSH configuration reads keys from
`/etc/ssh/lease_authorized_keys`, a root-owned file that starts empty in the
baseline snapshot. The controller only writes that fixed path. The provider's
file-write privilege itself is guest-wide, so keep its token within the
controller and scope it to the owned staging guest.

The API defaults to loopback. A non-loopback listener requires TLS 1.3 and a
certificate matching the registered endpoint. Each distinct staging bearer
token identifies one task principal. Lease IDs are checked against that principal on every read or
mutation. The request key is scoped to that principal. Run with
`python3 allocator/pilot.py CONFIG.json`. Keep the SQLite database on persistent
storage outside the leased guest. Keep secrets in private files. Set the
following configuration keys:

- `database`, `client_tokens` (principal to private token file), `guest_user`, and `guest_host_key` (host public key verified independently).
- For a LAN endpoint, `listen`, `port`, `tls.cert_file` and `tls.key_file`.
  Keep the private key on the controller. Give the authorized task the
  independently obtained CA and its own bearer credential file.
- `proxmox.url`, `proxmox.node`, `proxmox.guest_id`, `proxmox.guest_name`,
  `proxmox.snapshot`, `proxmox.token_file`, and `proxmox.ca_file`
- `capacity.token_file` (separate read-only token), `capacity.storage`,
  `capacity.minimum_memory_mib`, `capacity.minimum_storage_mib`,
  `capacity.other_reserved_memory_mib`, and `capacity.protected_guest_ids`.

The capacity reader needs audit access to the node, its VM/LXC inventory,
and the selected datastore. It has no provider mutation rights. The controller
uses the smaller of physical available memory and total memory less running
and protected guest commitments. It then subtracts other reserved memory and
the slot's configured allocation. Storage must have room for the slot's
maximum virtual disk writes above the host floor. Missing protected guests,
invalid telemetry, or a failed read cause an immediate rejection. A rejected
request leaves no lease or start task. The ledger serializes only this pool's
requests. Keep out-of-band work coordinated and configure other reservations
explicitly until shared reservation discovery is implemented.

| Request | Body or result |
| --- | --- |
| POST /v1/leases | Task name, request key, recipe `debian13-amd64`, duration in seconds, and `ssh_public_key` (OpenSSH Ed25519 with no comment). Immediate capacity rejection, or a recorded lease ID. |
| GET /v1/leases/{id} | Inspect the journal and advance observed provider tasks. Ready leases include the guest's reported SSH endpoint after its key is installed. |
| POST /v1/leases/{id}/renew | Duration in seconds. Extends an active lease from now without shortening its existing deadline. |
| POST /v1/leases/{id}/release | An explicit `discard_disposable_data: true` for pilot data. Stops the owned slot and restores its clean snapshot. Poll inspect until released. |

An expired lease is overdue and keeps the slot reserved. Provider intent is
saved before issuing a mutation, together with recent task IDs of the same
kind. If the reply is lost, inspection can recover the exact task from
Proxmox history. A recovery match must be new and unique, and belong to the
dedicated token, node, guest and operation type. With no unique match the slot
stays held. The controller does not reissue the write. Reserve the provider
token exclusively for this controller.

Each lease installs only its caller's public key, and release must finish
restoring the clean snapshot before the slot accepts another lease. Verify the guest host key through the trusted provisioner before setting
`guest_host_key`. Use the returned `host_key_alias` with strict verification,
so a DHCP address change does not weaken the host identity check. Automated credential issuance, lifecycle registry projection, automatic discovery of
reservations outside this controller, and unattended checkpoint export remain open. Lost
provider task recovery is qualified for the single dedicated slot. Use only disposable
qualification data in the slot.

## Task-side discovery and use

The operator registers the private HTTPS endpoint as `endpoints.lease_api` on
an active record with capability `lease-pilot`. Treat this as a staging-only
interface. Obtain the trust material and the task credential through the
authorized operator path before accessing the endpoint.

```sh
python3 allocator/client.py --record <record-id> --registry-client <registry.py> \
  --ca <ca.pem> --token-file <task-token> acquire --task <task> \
  --request-key <unique-key> --seconds 900 --ssh-public-key <key.pub>
```

Use the same options with `inspect <id>`, `renew <id> --seconds 900`, or
`release <id> --discard-disposable-data` for qualification data. The client
returns JSON, with structured rejection details on stderr and exit status 2.
Capacity rejections require a new caller decision. The client never retries
mutations automatically.


## Opt-in staged reclamation

Set `reclamation.verifier_token_file` to a separate private bearer credential.
The default `reclamation.grace_seconds` is 7200. New requests may opt in with
`--reclaim-on-expiry`. Previous leases keep their original policy. An opted-in
lease remains renewable during grace. After grace the controller stops the guest
and holds its disk while the exporter obtains and verifies a checkpoint. Early
`release <id> --preserve-checkpoint` starts the same process. An opted-in
lease cannot be reset through the discard option.

The owner can set `hold <id> --mode disk --reason <reason>` or explicitly
preserve the running guest with `--mode running`. A disk hold permits shutdown
and prevents reset even after backup. Use `hold <id> --mode clear` to remove
the hold.

The separate verifier uses `GET /v1/final-export` to discover pending work and
`GET /v1/final-export/{id}` to inspect the exact lease, guest and challenge. It uses `POST` on that route with
the challenge and one of these outcomes.

- `failed` with a short reason. The disk stays reserved and the error is
  visible to the owner. After 24 hours the pending export requires owner
  review. The owner can use `retry-export <id>` after repairing the cause.
  Retry resets the failure window and invalidates previous challenges.
  After 24 hours the owner can also record `retain-failed-export <id>
  --reason <reason>`, or explicitly authorize discard with
  `discard-failed-export <id> --confirm-lease <id> --reason <reason>`.
  The controller journals the exact discard decision with its reset intent.
- `verified` with the full encrypted repository snapshot ID, expected
  `/lease-{id}.tar.gz` path, and independently downloaded SHA-256. The
  controller checks the stopped guest and its observed start history, records
  the receipt, and completes rollback through the same durable task journal.
  Repeating the exact receipt is safe. Inspect returns the checkpoint path,
  snapshot and digest after release.

Use a dedicated slot under exclusive controller control. Proxmox task history
is a supplementary consistency check. It cannot fence host-root actions or
writes to a stopped guest disk. The pilot expects the verifier to download and
check the archive through a trusted storage integration before posting.
The 30-day retain-through timestamp records the policy. Notification delivery
and delegated reviewer identities remain open.


### Backup worker

Run `python3 allocator/exporter.py CONFIG.json` once per scheduling tick.
Configure the private controller `endpoint`, `ca_file`, scoped
`verifier_token_file`, private `lock_file`, and the owned `guest_id`.
Supply `capture_command` and `restic_command` as argument arrays. The capture
command receives the guest and lease IDs. The operator-provided adapter must
independently verify the exact stopped guest and export only its declared
workspace and evidence. The backup adapter runs restic against the intended
encrypted repository without exposing repository secrets to task agents.
The worker applies configurable `max_archive_bytes` and `command_timeout`
limits, looks up existing snapshots by export challenge, downloads the archive
to compare SHA-256, and submits a receipt only after that succeeds. A failed
attempt leaves the disk reserved. Use one scheduler for this slot.

The staged worker can run under systemd. Provisioning a durable least-privilege
capture and backup transport for stable is still required. The qualification
uses operator-owned forwarding that never grants provider or backup credentials
to the worker. Deploying this worker alongside a storage service or allowing
task agents to retrieve artifacts independently requires a scoped adapter.


### Retention, recovery and delegated renewal

Owners can use `pin <id> --reason <reason>` and `unpin <id>` for a released
checkpoint created by the backup worker. For automatic expiry, enable
`purge_expired_checkpoints` in the controller's `reclamation` configuration
and the worker configuration. Use this only with a backup policy that protects
these snapshots from external pruning while the allocator retains them. The
worker checks the exact snapshot, lease path and tags before forgetting it.
It does not prune repository-wide data. Expiry stays outstanding until the
worker independently confirms the snapshot is absent. Metadata remains.

Set `recovery.endpoint` and `recovery.worker_token_file` in the controller.
Run `recovery.py` with the private TLS listener settings, controller HTTPS
endpoint and CA, the separate recovery worker token, and a read-only Restic
adapter. Recovery never receives the task's general credential. An owner can
run `recover <id> --output <archive.tar.gz>` after release. A short-lived
single-use ticket grants only the exact checkpoint. Both sides verify the
archive digest. The client saves it privately and refuses to overwrite
existing files. A valid recovery ticket defers expiry for a short window.

The owner can issue a task runner a renewal-only credential with
`delegate-renewal <id> --duration-seconds <window> --extension-seconds <step>
--write-token <private-file>`. A timer or job uses that token with
`delegated-renew <id>`. An authorized owner can rotate or revoke it.
The runner has no access to task inspection, SSH, artifacts or release.
It cannot extend a lease past the declared delegation window. Owner renewals
remain independent and may keep a lease active beyond that window.


### Incremental checkpoints

Set `recovery.accept_uploads` in the controller configuration and
`accept_uploads` in the recovery worker configuration to enable uploads.
Set the recovery worker's `restic_command` to the read role and
`upload_command` to the write role. Uploads use the writer to store and
verify the checkpoint. Each owner download uses the reader. For existing
single-adapter installations, omitting `upload_command` keeps the original
command for both operations. Leave uploads disabled for a read-only worker.

Have the task runner publish a consistent, immutable workspace archive at
its declared checkpoint interval. Submit it with
`checkpoint <id> --archive <archive.tar.gz> --request-key <checkpoint-key>`.
Use a new key for new bytes and the same key when retrying a transmission.
When the key is omitted, the client uses the digest. A periodic job can
submit the current completed archive without creating another snapshot
for unchanged bytes.
The controller issues a short-lived grant for the declared size and digest.
The worker reads the stored archive back before acknowledging the checkpoint.
A failed upload leaves the existing acknowledged versions available.

Inspect the lease for recent checkpoint IDs, request and verification times,
and errors. Use `recover <id> --checkpoint-id <version-id> --output <file>`
with an acknowledged version, including while a failed final export is held.
Use the same `--checkpoint-id` with `pin` or `unpin` for that version.
The same 30-day post-release retention policy applies to each version.
The calling task runner owns archive consistency and checkpoint cadence.
The pilot does not infer a consistent copy of files while they are changing.


### Shared capacity reservations

Configure `reservation_tokens` with a separate credential for each
orchestration principal. Use `reserve-capacity` with a request key,
purpose, memory in MiB, and storage in MiB. Use `list-reservations` and
`release-reservation <id>` with the same principal. A reservation
persists until that principal releases it. Task credentials cannot
modify reservations.

Both lease admission and reservations check live host capacity and the
same SQLite ledger. Requests that exceed protected headroom are rejected
immediately with a memory or storage reason. Multiple hosts and deployers
that act outside this allocator still need a shared admission authority.

### Restricting backup access

Run `restic_scope.py` on the storage side with a private configuration
that sets `role` to `read`, `write`, or `expire` and binds the underlying
`restic_command` to a dedicated encrypted allocator repository. Use
separate credentials for the three command endpoints. Keep repository
secrets on the storage side. The writer can upload and verify checkpoints.
The reader can dump a verified allocator archive. The expiry credential
can remove an exact managed snapshot and check the repository index.
Set the exporter's optional `retention_command` to the expiry endpoint.
The standalone restic role uses a trusted deployment configuration.
Never allow a caller to supply that configuration or repository argument.

For an SSH adapter, bind each key to `restic_scope.py <private-role-config>
--ssh-original-command` with OpenSSH's `restrict` and `command` key options.
Configure the role and repository on the storage side. Each forced command
parses arguments as data and refuses shell and out-of-role commands. Use
independent OS accounts for the deployed roles when their repository
credentials differ.


### Task credential issuance

Configure `task_issuer_tokens` with one secret file per trusted job launcher.
The launcher issues a scoped credential with `issue-task-credential
--task-ref <task-ref> --seconds <duration> --write-token <private-file>`.
Give the task only the issued credential, its own SSH key and the CA.
The launcher cannot acquire, inspect or operate task leases with its issuer
credential. Each launcher owns a separate task namespace.

Issue again for the same task reference to rotate access without changing
ownership or the existing lease. `revoke-task-credential --task-ref
<task-ref>` revokes controller access and delegated renewal for that task.
Guest SSH stays bound to the active lease until its separately verified
provider cleanup. Task credential expiry does not delete the guest.

### Cold capture for an ext4 Linux slot

Install `export_slot_disk.pl` on the Proxmox host as an operator-owned forced
command. Bind its five arguments to one VM ID, guest name, SMBIOS UUID, exact
LVM volume and maximum raw byte count. The source identity may call only
`read-slot`. It can read the entire stopped slot disk, so give this identity
only to the trusted capture worker. The provider holds the Proxmox config lock
until the compressed raw stream finishes or times out. It opens no guest
filesystem. Administrative storage writes outside Proxmox still require
operator coordination.

On the isolated worker, use `cold_capture.py CONFIG GUEST LEASE` as the
exporter's capture command. Install Debian's `python3-guestfs` and its QEMU
appliance there. Configure `source_command` as an argument array for the
pinned SSH command, `spool_dir` as a disposable private directory,
`disk_bytes` as the exact raw size, and `max_spool_bytes` as a bounded
temporary allocation. Set `guest_id`, the absolute `workspace` and optionally
`appliance_memory_mib`. Allow for transfer plus appliance boot in the export
timeout. Use a service-managed temporary directory that is cleaned on worker
termination. A stale or full spool must leave the lease held.

The reader checks stream completion and size before opening the explicit raw
image read-only in a network-disabled appliance. The supported recipe has one
x86-64 Linux root GPT partition with ext4. The reader checks the protected
lease identity written at admission before archiving the declared workspace.
A missing or mismatched identity, unsupported layout or unreadable workspace
fails the export and preserves the stopped slot. Leases predating the marker
need operator review. Lease owners should place their work and evidence in the
declared workspace. OS data elsewhere on the disk is outside the archive.

### Operator lease review

Set the controller's optional `review_token_file` to a dedicated secret
readable by the controller and its authorized dashboard. Configure the
dashboard's private `lease-review.json` with the allocator HTTPS `endpoint`,
its `ca_file`, and a private `token_file`. The dashboard offers
the **Leases** tab at `/#leases` behind its usual operator login. Existing
`/leases` bookmarks open the same tab. Current shows leases through final export and reset.
Released leases are available under History. The reader can view ownership,
renewal deadlines, review holds, export failures, available checkpoint counts
and the latest checkpoint's verification and retention dates. Requests are
paged and made when the operator opens, refreshes or expands the history.
The review credential cannot issue leases, retrieve artifacts or change holds.

### Capability admission and reviewer holds

A task can call `capabilities` with its ordinary credential to inspect the
qualified recipe and the slot's currently observed vCPU, memory and disk size.
Pass `--requirements '{"os":"linux","architecture":"amd64","memory_mib":512}'`
to `acquire`. Numeric requirements are minimums. Unsupported capabilities
and insufficient headroom return an immediate, specific rejection. Replaying
a request key with different requirements is rejected. A capability report is
an observation and reserves no capacity. Pin `proxmox.guest_uuid` and
`proxmox.guest_volume` together to check hardware and disk identity before
provider changes.

The owner can designate another active task principal using
`assign-reviewer <lease> --reviewer <principal>`. The reviewer can call
`reviewer-hold` with a mode and reason. Owner and reviewer holds are
independent and persist across restarts. The reviewer receives no guest,
artifact or release authority. To override an unresolved reviewer hold, the
owner must confirm that exact reviewer and give a reason. The recorded
override preserves the affected principal, prior reason and data disposition.

### Matching replacement and workspace restore

Use `resume <predecessor> --task <task> --request-key <new-operation-key>
--seconds <duration> --identity <new-private-key>` with the owner's ordinary
credential. The task client requires the adjacent `restore_workspace.py`
helper and a fresh Ed25519 identity. The controller records the predecessor,
requirements and chosen acknowledged checkpoint on the replacement. The
default selects a retained final checkpoint, otherwise the newest available
incremental one. Use `--checkpoint-id` for a particular incremental version.

The client verifies the archive on download and again in the new guest before
installing it in the task's workspace. An existing nonempty workspace is
preserved. Repeating the same request reports the existing restore and leaves
any subsequent edits intact. The result identifies the captured scope and
timestamp. Files outside that scope and changes after an incremental
checkpoint are outside the recovery point.
