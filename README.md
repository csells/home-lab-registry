# Home Lab Registry

A small, self-hosted machine and service catalog with an authenticated dashboard. Machines and services have separate views. Machine cards put compact CPU, RAM and disk history bars above browser SSH, hypervisor, KVM and AMT access. Device and host-console credentials are separate; copy confirmation stays inside the button. Services show health freshness, placement and configuration-backup locations.

## Components

- `registry/`: Python standard-library HTTPS API, SQLite catalog, revisions, health observations, probes and export/restore.
- `dashboard/`: read-only dashboard, restricted SSH measurements, credential-copy endpoint, and Apache Guacamole session integration.
- `deploy/guacamole/`: pinned official containers and an example authenticated proxy.
- `skills/home-lab/`: portable copy of the operating skill, including registry discovery, Proxmox/community-script practices and backup procedures. Inventory belongs in the registry, never in the skill.

This repository contains source and examples only: no real catalog, machine identities, addresses, credentials, private keys, certificates, screenshots or operational backup archives.

## Deployment

Use a dedicated Linux VM with Python 3.11+, Docker Compose and an authenticated HTTPS reverse proxy. Budget at least 2 GiB RAM for registry, dashboard and Guacamole; actual capacity depends on session count. Install Python dependencies from `requirements.txt`. Run the registry with `python3 registry/registry.py --config /etc/lab-registry/config.json`; inspect `--help` and the config fields at the end of that source. Supply TLS certificate/key, database path and independent reader/writer tokens privately. Bind/route the API only to trusted clients. Run the dashboard using `python3 dashboard/server.py` behind your proxy on loopback 8089. Example service-manager units are in `deploy/`; create their unprivileged accounts and make private files readable only by the appropriate service account.

Dashboard `private/` files (never committed):

| File | Contents |
|---|---|
| `login.json` | `username`, `password` matching the outer dashboard authentication |
| `reader.json` | `url`, `ca_file`, `reader_token`; read access only |
| `registry-ca.pem` | Registry CA, referenced by reader config |
| `origins.json` | JSON array of exact permitted HTTPS origins |
| `targets.json` | Map of machine IDs to `{host,user,kind}`; kind `proxmox` or `macos` |
| `metrics_key`, `known_hosts` | Restricted-command SSH key and independently verified host pins |
| `terminal_key`, `guacamole_known_hosts` | Dedicated interactive SSH key and verified host pins |
| `guacamole-key` | Same signing key as private Guacamole `.env` |
| `machine-passwords.json` | Map of machine IDs to login IDs and passwords |

The measurement key must execute only fixed read-only commands. Proxmox expects three JSON lines: node `/status`, node `/storage`, and `/cluster/resources --type node` via `pvesh`. macOS expects `top` CPU output, `sysctl hw.memsize kern.boottime`, `vm_stat` and `df` for the Data volume. Provision these explicitly and pin each host identity. The browser never supplies shell commands to this collector.

See the Guacamole README for terminal deployment. Preserve Authorization through the reverse proxy to the dashboard; authenticate Guacamole at that same outer boundary. Restrict origins and enable WebSockets. Do not expose the loopback dashboard or Guacamole backend directly.

## Registry records

Use `registry/client.py --help`. Records require name, host, owner, capabilities, endpoints, lifecycle, runbook and dependencies. Machine records include capability `machine`, `resources.kind: machine`, hardware, and `resources.management_access` entries with `id`, `label`, `url`, `username`, and `password_available`. SSH entries use ID `os` and `browser_terminal: /terminal/<host>/`. Optional `resources.console_login` supplies username, credential_id and password_available for host-console login. Secret values belong only in the private mapping. `config_backup` records status, locations and notes. See tests for schema examples.

Writes require writer authentication and `If-Match` revision preconditions. Readers cannot mutate the catalog. Health observations have expiry; unknown/stale is not proof of downtime. HTTP/TCP probes and local exporters are operator-configured; this is not automatic network discovery.

Live charts keep bounded in-memory history, sampled on demand while a machine card is visible, shared between viewers. Charts display a 10-minute window; unavailable periods have gaps. History resets on dashboard restart. Disk chart shows the most-used storage pool, not disk throughput. CPU values reflect the host collector's own sampling cadence. Unconfigured machines show no fabricated readings.

## Security and recovery

Designed for a trusted private lab, not multi-tenant hosting. Dashboard users can open privileged shells and retrieve configured passwords. HTTPS, strong dashboard authentication and tightly scoped network access are required. Passwords never enter rendered page text, but the clipboard, developer tools and recipient application can access copied secrets. Keep backups encrypted and independently recoverable. Site-specific probes belong outside public source.

Take a SQLite-consistent export and back up private configuration, proxy, service units and terminal keys before deployment. Keep a rollback release until actual public login, credential copying, terminal commands and health refresh pass. The included home-lab skill describes centralized encrypted Drive backups; adapt the destination to your environment rather than treating documentation as proof of configured backups.

## Tests

```sh
(cd registry && python3 -m unittest -v)
(cd dashboard && python3 -m unittest -v)
node --check dashboard/app.js
```

Browser qualification should include real clipboard comparison without printing values, real terminal commands, phone-width layouts, live sampling, anonymous/cross-origin rejection and stale/failure states. Desktop emulation does not prove physical mobile keyboard behavior.
