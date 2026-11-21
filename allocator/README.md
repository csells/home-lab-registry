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

The API binds to loopback and binds each distinct staging bearer token to one
task principal. Lease IDs are checked against that principal on every read or
mutation. The request key is scoped to that principal. Run with
`python3 allocator/pilot.py CONFIG.json`. Keep the SQLite database on persistent
storage outside the leased guest. Keep secrets in private files. Set the
following configuration keys:

- `database`, `client_tokens` (principal to private token file), `guest_user`, and `guest_host_key` (host public key verified independently).
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
so a DHCP address change does not weaken the host identity check. Distributed credential issuance, a
supported client transport, registry projection, automatic discovery of
reservations outside this controller, checkpoint export and automatic expiry
cleanup remain open. Lost
provider task recovery is qualified for the single dedicated slot. Use only disposable
qualification data in the slot.
