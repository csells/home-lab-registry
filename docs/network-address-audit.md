# On-demand network address audit

The dashboard's **Check IP reservations** button starts a read-only deterministic
job. Loading the page never starts a job. Concurrent clicks share the running
job; a 30-second cooldown prevents accidental duplicate scans. Status polling
ends when that job finishes. No model or recurring agent session is involved.

`dashboard/private/network-audit.json` contains operator-owned configuration:

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
address/MAC match. Unsupported profiles, unavailable guests and appliance
configuration are explicit gaps, not a passing fleet audit. A passing address
is not proof that every configuration path on its machine was examined.

Backend private configuration must be mode 0600, and its containing directory
0700. Retain dashboard authentication and origin checks. Audit responses expose
addresses only to authenticated dashboard users, never credentials or raw files.
Candidate work and tests run in a disposable VM; production promotion still
requires approval.

Run tests in the candidate VM with:
`python3 -m unittest discover -s tests -p test_network_audit.py`.
