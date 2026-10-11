# Address registration and consumers

The collector reads one configured LAN interface every 30 seconds. It announces
immediately at startup, on address change, and every two minutes thereafter.
This is independent of viewer subscriptions and GUI login. Metrics remain local
without viewers; registration transmits only the address and interface. Failed
registration retains metrics and retries. The registry marks presence stale after
five minutes; stale presence is not proof of host health.

Configure `registration` in the private collector configuration:

```json
{
  "registration": {
    "url": "https://lab.example.net/v1/machines/machine-example/presence",
    "interface": "eth0",
    "token_file": "/etc/lab-metrics/registration-token"
  }
}
```

`ca_file` is optional for a private registry CA. Use a stable, publicly resolvable
HTTPS registry URL or a maintained internal DNS name; never its current DHCP IP.
Do not put a browser's Basic-auth gate in front of the machine route. The registry
itself authenticates it using a separate per-machine random token. Store only its
SHA-256 digest in private `machine_enrollments` settings, with `allowed_networks`
and a stable `tls_name`. A machine token cannot read the catalog, mutate service
specifications, or announce another machine.

For every new address, the registry verifies the collector using TLS 1.3, its
pinned CA and the enrolled DNS certificate identity. Configure `presence_tls`
with `ca_file`, `cert_file`, and `key_file` for the registry's client identity.
The agent server certificate must contain the stable machine DNS identity in SAN.
Dashboard `metrics_stream.server_name` uses that identity when connecting to the
current address. Do not disable certificate verification to accommodate DHCP.

Machines select the fields they own through `resources.registration`:

```json
{"endpoint_keys":["ssh","proxmox"],"access_ids":["os","proxmox"],"metrics":true,"probe":true}
```

A service selects its actual guest, independently of its physical parent, using
`resources.address_bindings`:

```json
[{"machine_id":"machine-example","endpoint_keys":["health"],"probe":true}]
```

Only selected endpoint keys, login paths, metrics URLs and probe addresses change.
Public URLs and external KVM/AMT addresses are independent. Inventory clients must
fetch resolved records from the registry, not retain a copied IP. Presence is
stored separately from operator specifications; heartbeats do not create revision
history. Encrypted database backups retain presence; exported specifications do
not treat transient presence as operator configuration.

For browser SSH, set each login route's `host_key_alias` to an immutable identity
and provision that alias in the private `guacamole_known_hosts` file through a
trusted channel. Guacamole receives the pinned host key with every connection;
address changes do not trigger trust-on-first-use. Never pin a newly discovered
key merely because its IP matches an announcement.

`backup/collect.py` resolves the registered source before every export and requires
current presence. Its manifest uses `machine_id`, `username`, optional `port` and
`filename`, instead of `user@IP`. The backup reader configuration is private at
`private/registry-reader.json`; the SSH known-hosts alias must equal `machine_id`.
The export command remains constrained to the existing authorized `export` action.
Failure to resolve, stale registration, or a wrong SSH key fails the backup; it
must not back up a different computer that acquired the old address.

## Bootstrap and appliances

Keep a public tunnel's origin loopback-local, Caddy listeners independent of a
specific leased IP, and the dashboard's API connection loopback-local with a valid
stable certificate identity. This lets the registry return after its own DHCP
change before any agent can reach it. Test that bootstrap separately from an
ordinary agent move.

KVM and AMT appliances do not run this daemon. Their addressing requires separately
verified stable reservations or authenticated discovery. Never rewrite their
addresses from the host OS announcement. Similarly, an appliance OS such as HAOS
needs supported guest discovery or a supported agent installation; do not claim
coverage based solely on its physical parent's daemon. Offline machines cannot
be enrolled or validated until they return.

## Qualification

Run unit tests and `tests/registration_e2e.py` inside a disposable Linux VM only.
The end-to-end test requires root and `DISPOSABLE_VM_TEST=1`: it creates a dummy
interface, real certificates, a system service, and an isolated registry, changes
the interface address, verifies discovery/streaming and exports through real SSH.
It restores its temporary SSH authorization and removes its test interface.
The test prints measured idle CPU/RAM overhead and cleans up its runtime.

Deployment must include every consumer's registry bindings, trusted identity pins,
boot service, encrypted configuration backup, and post-restart verification. A
successful test is not evidence that the production fleet was deployed.
