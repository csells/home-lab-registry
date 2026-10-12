# GPU history

GPU utilization accompanies each five-second sample in the bounded ten-minute
history. The browser uses the same authenticated, demand-driven stream as CPU,
RAM and disk. A fourth purple chart sits in the top resource strip.

The `gpu` object has `status`, nullable `percent`, and a short `note`:

- `available`: a measured utilization percentage, including genuine zero.
- `not_applicable`: no GPU accelerator exposed to this OS. The chart has an N/A overlay.
- `unavailable`: detection/counter failure, unsupported driver, missing privileges,
  or an older collector with no GPU field. Show Unavailable, never invented zero or N/A.

A GPU must be exposed to the guest to provide utilization. A virtual display
adapter alone (QXL/Bochs/simple framebuffer) does not count. In an explicitly
authorized LXC sharing a render device, GPU utilization is host-wide; CPU, RAM
and disk readings remain container-scoped. Label this distinction in staging.

## Sources and permissions

- Linux AMD: the kernel's `gpu_busy_percent` sysfs interface.
- Linux Intel i915: persistent `perf_event_open` descriptors for the PMU's busy
  engine counters. Delta busy nanoseconds divided by delta running nanoseconds;
  render, copy and video engines are included. Uses the busiest engine rather
  than summing overlapping engines. Access depends on the kernel's perf policy;
  root/CAP_PERFMON may be required. Do not weaken global perf policy to hide an error.
- macOS: bounded, two-second-timeout `ioreg` inspection of IOAccelerator device
  utilization. Uses the maximum reported device percentage. This property is an
  observed Apple-driver interface, not a promised stable public API; missing
  statistics explicitly show Unavailable.
- Other drivers/platforms: explicit Unavailable until implemented and verified.
  NVIDIA/Windows monitoring is not implemented in this candidate.

These are vendor utilization measures, not a cross-vendor benchmark. GPU memory
is not included. Linux descriptors stay open until collector exit; hardware or
permission changes require collector restart. No external polling agent, process
scan, or `intel_gpu_top` subprocess per sample is needed.

References: [AMD sysfs](https://kernel.org/doc/html/latest/gpu/amdgpu/thermal.html),
[Intel GPU tools](https://gitlab.freedesktop.org/drm/igt-gpu-tools),
[Linux perf events](https://man7.org/linux/man-pages/man2/perf_event_open.2.html).

## Candidate validation

- CI passed Go race tests, Linux build tests, Apple Silicon cross-build,
  registry/dashboard regressions, and actual Chromium rendering at393px/1440px.
- Browser fixtures verified changing GPU bars, genuine0%, N/A, failed counters,
  and backward compatibility with collectors that omit GPU data.
- Disposable Linux VM: actual collector -> mTLS stream -> dashboard -> rendered
  mobile card passed. Four charts, advancing samples and N/A for the guest with
  no GPU. Fixture readings are not presented as measurements of a physical GPU.
- An approved disposable unprivileged LXC sharing an AMD render node passed a
  real headless rendering test: utilization rose from 0% to 87–95%, then returned
  to 0%. The host remained reachable. The actual collector -> mTLS -> dashboard
  VM -> Chromium flow passed at 393px and 1440px, including cached load history.
- A one-minute idle measurement of that collector used 0.022% of one CPU core
  and 9.44 MiB of memory. This is a measured idle case, not an overhead guarantee
  across hardware, platforms or workloads.
- Apple parser was exercised against captured real IOAccelerator output; Intel
  PMU event/type/unit availability and AMD sysfs availability were observed on
  the lab hosts. The candidate collectors have NOT been run on those physical
  hosts yet. Native macOS and Intel PMU under-load validation remain pending.

No production promotion has occurred. GPU collection on Windows/NVIDIA is not
implemented. Do not claim the <1% overhead target until hardware measurements pass.

## Repeatable bounded GPU load

On an authorized disposable Linux GPU test environment with Mesa EGL/GLES2
development packages and access to its render device:

```sh
cc -O2 tests/gpu_load.c -lEGL -lGLESv2 -o /tmp/gpu-load
timeout --kill-after=3 30 /tmp/gpu-load 20
```

The test rejects Mesa software renderers and runs for at most 60 seconds
(default 20). Observe utilization before, during and after the load through the
actual dashboard stream. Preserve output and verify host reachability. Do not
reattempt failed PCI passthrough merely to run this test; a shared render device
in an explicitly approved disposable LXC is an alternative with different
isolation and measurement scope.
