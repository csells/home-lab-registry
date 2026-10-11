# Cached streaming resource metrics

The Go agent uses maintained [gopsutil](https://github.com/shirou/gopsutil) for OS
measurements. It samples CPU and memory every five seconds, reads configured
filesystem capacity and optional LVM thin pools every minute, and keeps 120 real
samples in RAM. Sampling continues without viewers; resource samples are transmitted only with
a subscription. Optional address registration sends a small heartbeat separately. There are no per-sample file writes. Restart starts a new history;
gaps are shown honestly. Charts use explicitly labeled ranges: CPU starts at zero with a minimum 10-point
span, RAM zooms to a minimum 10-point range, and disk capacity stays at 0–100%.
Current values retain one decimal and the live caption shows the sample time.
Disk readings carry their own timestamps. RAM means total
minus OS-available memory, excluding reclaimable cache.

The dashboard discovers enabled agents from the registry. It opens one shared,
mutually authenticated HTTPS stream per watched machine. Browsers receive
same-origin authenticated Server-Sent Events: cached history first, then updates.
Hiding the tab, scrolling past the card, switching to services or leaving closes
the subscription. The final disconnect releases the upstream within the heartbeat
window (normally 5–10 seconds; transport failure timeout 15 seconds). No SSH jobs,
polling requests, or forwarding processes run on the measured machine.

## Build and Linux installation

Build in a disposable environment using the pinned Go module and checksums:

```sh
cd agent
go test -race ./...
go build -trimpath -ldflags='-s -w' -o lab-metrics .
```

After staged approval, install the binary at `/usr/local/bin/lab-metrics`, private
configuration at `/etc/lab-metrics.json`, and the example systemd unit. Select the
actual local filesystem paths and enable `lvm_thin_pools` when appropriate. Do not
sample network-mounted filesystems. The memory cap is a guardrail; benchmark the
actual destination idle and streaming as well.

LVM reporting invokes the fixed reporting operation `lvs` once a minute. It reports
thin-pool physical allocation and metadata percentage, separately from filesystem
usage. It does not add guest virtual disk sizes together. Do **not** add `--readonly`
to `lvs`: that mode can omit live allocation percentages. Missing permissions or
failed measurements create explicit warnings. Root is unnecessary if LVM reporting
is disabled; run as a dedicated unprivileged account in that case.

## Authentication and registry discovery

Generate a private metrics CA and issue separate server and dashboard-client
identities. Each server certificate must have its exact registry DNS name/IP in
SAN, `serverAuth` extended key usage, and a private key readable only by its service
account. The dashboard certificate requires `clientAuth`. Never ship CA private
keys to agents or the browser. Provision and verify identities out of band before
opening network listeners. The agent requires TLS1.3 and a valid client certificate;
there is no anonymous network endpoint or plaintext fallback.

Set the agent's `listen`, `cert`, `key` and `client_ca` fields. Limit network access
to the dashboard where host firewall policy permits. An empty `listen` disables
network access, retaining only the mode0600 Unix socket for local diagnostics.

Dashboard private files:

- `stream-ca.pem`: agent CA trust anchor.
- `stream-client.pem` and `stream-client-key.pem`: dashboard client identity; key mode0600.

Enable discovery in the machine's existing registry record:

```json
{"resources":{"metrics_stream":{"enabled":true,"url":"https://node.example:9911/stream"}}}
```

Use the record's real endpoint. The dashboard verifies server identity, sends its
client certificate, refuses redirects, and accepts only the exact `/stream` path.
Missing collectors do not silently fall back to polling. Installation/version,
certificate expiry/renewal responsibility, measured overhead and current health
belong in that machine's record. Provision renewal and warn ahead of certificate
expiry; an expired identity must fail closed.

Local diagnostic snapshot (service-account access required):

```sh
lab-metrics -mode snapshot -socket /run/lab-metrics/agent.sock
```

It includes cached samples, a sequence number and active collector subscriptions.
It is not exposed over the network.

## macOS

Cross-build with `GOOS=darwin GOARCH=arm64 go build` for Apple Silicon (amd64 for
Intel). Use `/System/Volumes/Data` for the APFS data volume and disable LVM. Set a
writable Unix-socket path using `-socket`. Run under a dedicated account using a
launchd daemon with that `UserName`, rather than depending on interactive login.
Precreate its private socket directory with the proper owner. Cross-compilation
alone is not macOS runtime or overhead qualification: measure in an isolated Mac
VM or during explicitly approved rollout before claiming that qualification.

The repository includes `com.home-lab.metrics.plist.in`, `install-macos.sh`,
and `config.macos.example.json`. Prepare a private bundle containing the approved
`lab-metrics` binary, `config.json` based on that example, and provisioned
`server.pem`, `server-key.pem`, and `ca.pem`. Then run:

```sh
sudo bash deploy/metrics/install-macos.sh SERVICE_USER /path/to/private-bundle
```

Use an existing unprivileged service account. The installer creates a system
LaunchDaemon with that account as its runtime user, so GUI login is unnecessary.
This does not bypass a FileVault preboot unlock. The installer replaces only the
collector's files and launchd job. Validate the installed daemon and stream before
recording rollout success. Never commit the private bundle, enrollment tokens,
real inventory, certificates, or keys. Source and portable deployment templates
belong here; machine-specific settings belong in the registry and encrypted backups.

## Reverse proxy, qualification and backups

Disable buffering for `/api/metrics/stream` and retain dashboard authentication.
Nginx: `proxy_buffering off; proxy_read_timeout 30s;`. The backend also sets
`X-Accel-Buffering: no`. Caddy: use `flush_interval -1` if necessary; put Origin
rejection inside a `route` block before proxy handlers so sorting cannot bypass it.

Validate cached history on first open, changing measurements without repeated
requests, one upstream for multiple viewers, no upstreams without viewers,
reconnection after interruption, explicit gaps/warnings, anonymous and foreign
origin rejection, mutual TLS (missing/wrong client and server identities), actual
filesystem/thin-pool usage, and measured idle/streaming CPU and RAM. Measure CPU
against one core, report the RAM denominator, duration and peaks, and distinguish
counters below timer resolution from true zero. One VM is not fleet qualification.

Back up configuration, certificates/keys, service units and reconstruction details
through the central backup service. History is disposable. Keep CA recovery
material encrypted in that same approved backup system, separate from agents.

See [candidate validation](VALIDATION.md) for measured overhead and qualification limits.
