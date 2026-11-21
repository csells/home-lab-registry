# Linux lease pilot

This controller is a disposable staging experiment with one pre-provisioned
Proxmox VM. The operator prepares a verified Debian 13 guest and a stopped,
clean disk snapshot. The controller receives a TLS-verified Proxmox token
scoped to that guest for status, guest-agent audit, power and snapshot rollback.
It cannot clone arbitrary guests or allocate host capacity. Configure the fixed
slot only after checking host headroom.

The API binds to loopback and uses a dedicated staging bearer token. Run with
`python3 allocator/pilot.py CONFIG.json`. Keep the SQLite database on persistent
storage outside the leased guest. Keep secrets in private files. Set the
following configuration keys:

- `database`, `client_token_file`, and `guest_user`
- `proxmox.url`, `proxmox.node`, `proxmox.guest_id`, `proxmox.guest_name`,
  `proxmox.snapshot`, `proxmox.token_file`, and `proxmox.ca_file`

| Request | Body or result |
| --- | --- |
| POST /v1/leases | Task name, request key, recipe `debian13-amd64`, duration in seconds. Immediate capacity rejection, or a recorded lease ID. |
| GET /v1/leases/{id} | Inspect the journal and advance observed provider tasks. Ready leases include the guest's reported SSH endpoint. |
| POST /v1/leases/{id}/renew | Duration in seconds. Extends an active lease from now without shortening its existing deadline. |
| POST /v1/leases/{id}/release | An explicit `discard_disposable_data: true` for pilot data. Stops the owned slot and restores its clean snapshot. Poll inspect until released. |

An expired lease is overdue and keeps the slot reserved. Provider intent is
saved before issuing a mutation, together with recent task IDs of the same
kind. If the reply is lost, inspection can recover the exact task from
Proxmox history. A recovery match must be new and unique, and belong to the
dedicated token, node, guest and operation type. With no unique match the slot
stays held. The controller does not reissue the write. Reserve the provider
token exclusively for this controller.

This pilot uses one trusted operator principal and an existing operator SSH
identity. Task-scoped guest credentials, registry projection, policy-driven
headroom across hosts, checkpoint export and automatic expiry cleanup remain
open. Lost provider task recovery is qualified for the single dedicated slot. Use only disposable
qualification data in the slot.
