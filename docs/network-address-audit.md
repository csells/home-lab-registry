# On-demand network address audit

The audit is a read-only deterministic job run on demand from an operator
machine, not from the internet-facing dashboard:

```sh
python3 dashboard/network_audit.py PRIVATE_DIRECTORY
```

It prints the result as JSON and exits nonzero when no result could be
established. The dashboard holds no audit credentials; it only displays the
latest run published to the registry. No model or recurring agent session is
involved.

`PRIVATE_DIRECTORY/network-audit.json` contains operator-owned configuration:

- `network_id`: router network identifier.
- `session_file`: protected eero session file, never delivered to the browser.
- `lan_cidrs`: household LAN ranges; isolated NAT/loopback/VPN are excluded.
- `targets`: registry machine IDs, SSH destinations, key and pinned known-hosts
  file names, optional host-key alias and passwordless sudo flag.

The backend fetches current eero reservations and the registry, then collects
Linux interfaces and saved network profiles through pinned SSH. It also attempts
running Proxmox guests using guest-agent execution or container execution. It
never changes network configuration, reservations or power state.

Results distinguish confirmed violations, verified addresses and unverified
coverage. A router address observed in a device list does not establish a
reservation. DHCP needs no reservation; static addresses require an exact
address/MAC match. Unsupported profiles and unavailable configuration are explicit gaps, not a
passing fleet audit. A passing address
is not proof that every configuration path on its machine was examined.

Private configuration must be mode 0600, and its containing directory 0700.
Results contain addresses, never credentials or raw files.
Candidate work and tests run in a disposable VM; production promotion still
requires approval.

Run tests in the candidate VM with:
`python3 -m unittest discover -s tests -p test_network_audit.py`.

## Extended verification and runtime state

The candidate includes authenticated readers for GL.iNet KVM, JetKVM diagnostics,
and Intel AMT Ethernet settings. Appliance URLs come from registry access records;
private adapter configuration supplies the protocol and credential references.
`appliance-credentials.json` stays server-side. GLKVM uses an explicitly provisioned
exact certificate pin, checked before sending HTTP credentials. A changed pin
fails closed. Expired pinned certificates are reported as warnings, not hidden.
Provision pins through a trusted access path; do not automatically trust a newly
observed certificate. HTTP-only appliance APIs must remain on the trusted LAN.

Home Assistant OS uses guest-agent execution of `ip` and NetworkManager tools,
without requiring Python. Ordinary running Linux guests use Python. Stopped
single-NIC Proxmox guests are inspected with read-only loop devices and ext4
`ro,noload` mounts; temporary mounts and volume activation are cleaned up.
Saved pending interface profiles are included. Cloud-init templates with no
applied profile report their saved Proxmox next-boot configuration separately.
Custom network snippets and unsupported disk layouts remain unverified.
Registered stopped macOS Tart guests use read-only APFS attachment and saved
SystemConfiguration. None of these readers boots a guest.

Results distinguish live verification, offline configuration verification,
addresses outside the household LAN, violations, and missing evidence. Offline
verification does not certify runtime behavior or arbitrary startup scripts.
Host Python needs PyYAML when inspecting netplan YAML offline. Appliance readers
need cryptography in the dashboard environment. SSH access is pinned; restricted
Mac access can use a pinned SSH jump host without relaxing authorized-key rules.

Guest power state on the dashboard comes from each machine's metrics agent
(`/guests` on its mutually authenticated listener), not from this audit or SSH.
See [guest inventory](../deploy/metrics/README.md#guest-inventory).

AMT may be unreachable from a VM on the same physical NIC as its management
engine. An AMT adapter can set `via_machine` to a configured, pinned SSH target
on another physical host. The read-only AMT request then runs there, with its
credentials sent over SSH stdin (never command arguments or a remote file).
Results record the vantage, and failure remains unverified.
