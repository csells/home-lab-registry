# Linux lease pilot

This controller is a disposable staging experiment with one pre-provisioned
Proxmox VM. The operator prepares a verified Debian 13 guest and a stopped,
clean disk snapshot. The controller receives a TLS-verified Proxmox token
scoped to that guest for status, guest-agent audit and file writes, power and
snapshot rollback. It cannot clone arbitrary guests or allocate host capacity.
Configure the fixed slot only after checking host headroom. Prepare a non-admin
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
supported client transport, registry projection, policy-driven headroom across
hosts, checkpoint export and automatic expiry cleanup remain open. Lost
provider task recovery is qualified for the single dedicated slot. Use only disposable
qualification data in the slot.
