# GPU history

GPU utilization accompanies each five-second sample in the bounded ten-minute
history. The browser uses the same authenticated, demand-driven stream as CPU,
RAM and disk. A fourth purple chart sits in the top resource strip.

The `gpu` object has `status`, nullable `percent`, and a short `note`:

- `available`: a measured utilization percentage, including genuine zero.
- `not_applicable`: no GPU accelerator exposed to this OS. The chart has an N/A overlay.
- `unavailable`: detection/counter failure, unsupported driver, missing privileges,
  or an older collector with no GPU field. Show Unavailable, never invented zero or N/A.

GPU passthrough must be visible in the guest to count as that guest's GPU. A
virtual display adapter alone (QXL/Bochs/simple framebuffer) does not count.

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

Pending disposable-VM runtime and rendered-browser checks. Lab hosts became
unreachable during development; no fleet deployment or production promotion has
occurred. Do not interpret the presence of this document as a passing validation.
Hardware-specific CPU/RAM overhead must be measured before claiming the <1% target.
