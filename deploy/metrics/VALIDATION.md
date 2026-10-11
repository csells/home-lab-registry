# Streaming metrics candidate validation

Validated 2026-10-10 in a disposable Debian 13 VM, 4 vCPU, 4096 MiB configured
RAM (3922 MiB visible to the OS). No production collectors were installed.

| Phase | Duration | CPU, one core | Peak RSS | Share of VM RAM | Stream bytes |
|---|---:|---:|---:|---:|---:|
| No viewers | 60 s | 0.0167% | 14.60 MiB | 0.3722% | 0 |
| Mutual TLS stream | 120 s | 0.0167% | 14.64 MiB | 0.3732% | 15,124 |

CPU accounting resolution was 0.01 seconds; these are short-window measurements,
not a guarantee for every machine. No physical disk reads/writes were attributed
to the agent during either window. Network bytes include initial cached history.
The benchmark used filesystem collection; LVM was validated separately below.
A later change added LVM stderr warnings and disabled LVM hint-file writes; it did
not change the benchmarked filesystem path.

Verified:

- Go race tests (3) and dashboard tests (7).
- Native TLS rejects missing clients, incorrect certificate purpose, untrusted CA
  and wrong server identity. Local diagnostics are not exposed over the network.
- Public SSE rejects anonymous requests, foreign origins and unknown machines.
- First browser open received 63 real cached samples in 0.63 seconds in this run.
- Desktop 1440px and phone-width 393px: visible history bars, no horizontal overflow.
  This was desktop browser emulation, not a physical phone test.
- A single persistent browser stream receives updates without resource polling.
  Two viewers share one upstream connection. Leaving the card closes the upstream.
- With no viewers, stream count becomes zero and the local sample sequence grows.
- The page visibility handler closes streaming and inventory requests, then
  reconnects with history on resume. Visibility events were simulated explicitly.
- Dashboard restart retains collector history; collector outage is shown as
  unavailable; collector restart begins fresh history without fabricating samples.
- Controlled CPU work in the disposable VM appears in the chart.
- Actual temporary LVM thin pool under the hardened unit: 256 MiB capacity,
  3.12% data allocation, 10.45% metadata allocation. Fixture cleaned up afterward.

Apple Silicon cross-compilation passes. macOS runtime and overhead qualification
remain pending an available isolated Mac VM. Cross-compilation is not runtime
validation. Host-level Proxmox qualification and fleet rollout also remain pending
staging approval; the tested agent measures the disposable VM itself.

## Flat-chart regression

The original fixed 0–100% axis compressed real idle CPU and memory changes to
0.4–0.5 rendered pixels at phone width. Whole-percent current values hid additional
variation. Collection was working; visual verification had been insufficient.

CPU now uses a zero-based scale with a minimum 10-percentage-point span. RAM
uses a labeled observed range with a minimum 10-point span and 5-point boundaries.
Disk capacity keeps its 0–100% scale: stable capacity must remain visually stable.
All scales are printed below the charts; current readings show one decimal, and
the live caption includes the sample time. No synthetic variation is added.

`dashboard/check_charts.py` tests the actual rendered SVG at phone width against
known changing and constant inputs, using an authenticated disposable preview.
Provide credentials through `LAB_LOGIN_JSON`; never include credentials in URLs.
This regression checks rendered pixel differences, not just sample arrival.

A bounded real workload in the disposable VM then exercised the complete path:
one CPU core busy for 30 seconds, 256 MiB allocated memory, and a temporary 2 GiB
filesystem reservation. At phone width, rendered bar-height differences were
38.7 px CPU, 17.9 px RAM and 2.8 px disk. Browser readings rose and returned after
cleanup. The fixture ran with a 384 MiB memory cap and 120-second maximum runtime;
its temporary file was verified removed. Desktop and phone-width overflow checks
passed. Idle variation also passed the original failing browser check after the fix.

## Address-registration candidate (2026-10-11)

Isolated Debian 13 VM, 2 vCPU / 2 GiB RAM; production services were not its runtime.
The `tests/registration_e2e.py` fixture used real TLS certificates, a systemd
collector, an actual dummy network interface changing from one IPv4 address to
another, and a real constrained SSH export. This tests the address-change event;
it does not claim a router DHCP lease-renewal or physical-host reboot test.

Passed: machine-scoped registration, disallowed identities/networks, stale-presence
handling, service endpoint and probe rebinding, unchanged public URLs, mTLS stream
reconnection with an unchanged DNS identity, rejection of the wrong TLS identity,
system-service restart without a user login, and backup export before and after
address change using an unchanged SSH host key. Browser-SSH connection generation
was unit-tested with stable key aliases; full Guacamole interaction across an IP
change remains a rollout validation item.

The final no-viewer interval measured 15.60 MiB RSS (0.790% of VM RAM). CPU accrued
no measurable ticks in 60 seconds (below the counter resolution, not proof of zero
CPU). These figures qualify this VM; verify actual fleet overhead after rollout.
The separate address heartbeat runs regardless of viewers; resource streams do not.

The candidate is not fleet deployment evidence. Production enrollment, dependent
bindings, appliance discovery/reservations, boot verification, and configuration
backup remain deployment requirements. Offline nodes cannot be marked complete.
