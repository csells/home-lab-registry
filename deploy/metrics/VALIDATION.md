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
