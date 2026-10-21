---
title: Render-node headless compositor backend
description: A third compd backend for offscreen testing and recording.
---

# Render-node headless compositor backend

Status: accepted, 2026-10-20.

compd has three backends: `kms-live` owns physical scanout, `nested` draws
inside a host compositor, and `headless` draws a virtual output using only
a render node. Headless is the offscreen test and future recording target;
hardware behaviour continues to be tested on KMS.

The headless backend lives beside nested at compd layer 4. GBM/EGL opens
the selected render node and verifies the EGL device identity. There is no
DRM device, master, connector, libseat, udev or libinput. The virtual output
is `headless-0`; mode and scale are explicit. Two offscreen GLES textures
alternate, with full repaints after scene damage. A one-shot software timer
caps frames at 60 Hz and stops while idle. Software presentation uses
CLOCK_MONOTONIC and claims no hardware completion or vblank.
`COMPD_HEADLESS_REFRESH_HZ` may select a lower rate from 30 through 60 Hz;
the virtual mode and frame timer use the same rate. The default is 60 Hz.

The binding profile is `headless`; the default Bus service is
`comp-headless`, with `comp-<name>` available through `--bus-service` for
instances. Headless never requests realtime priority or negative nice,
starts no XWayland, and leaves the desktop session environment alone.

Headless owns agent input authority without a VT; session locks still clear
agent input, while KMS and nested retain their session-pause rules. `seat0`
is advertised with no capabilities, and human input refuses `seat_unavailable`.
The agent pointer starts at output centre; absolute `headless-0` moves clamp
to the mode's logical bounds. Each Bus service has its own observation topics,
window registry and presentation statistics.

Screencopy and Bus captures run after completed offscreen rendering, including
on idle outputs. The normal picture excludes cursor elements; a second pass
is made only when a capture requests them. Cursor inclusion is chosen per
capture, including mixed requests in one frame, and an unavailable picture
fails its capture rather than substituting another cursor policy. Capturing
unchanged content does not count as a new presentation.

## Deployment and recording

An instance is a headless desk: its own nspawn machine, noded, session bus,
PipeWire and portald, with only the GPU render node bound and no VT, tty or
seatd. `compd-headless@.service` uses the session slot `%i`, just as `compd@`
does: a console desk uses a VT number; a headless desk can use `rec`. Both
read `/etc/mixos/session/%i.env`. Inside the desk, the compositor's Bus service
is `comp`; each desk has its own node. The standalone CLI default remains
`comp-headless`.

The component owns `mixos-headless.slice` with `CPUQuota=200%`; the desk's
compositor runs there at nice 10. Desk provisioning places its portal,
PipeWire, WirePlumber and encoder in that slice too. The container has its
own quota, so the slice does not need a per-instance name.
`COMPD_HEADLESS_REFRESH_HZ` passes through the session environment file.

portald's `serve --screencast-max-framerate N` sets the cast ceiling in
1..=120 Hz. It defaults to 30, preserving console sessions; a headless desk
can request 60 through its portal unit drop-in. This ceiling does not change
the compositor's refresh rate. These templates do not provision or start a desk.

Phase 4b acceptance measures presentation cadence: `presented_hz` is the
presented-frame count divided by the paced probe's elapsed time, and
`cadence_missed` counts presentation gaps greater than 1.5 requested intervals.
Captured-frame uniqueness is measured from the probe's pixel frame IDs.
The client-deadline `missed` count remains a regression guard only: two
free-running 60 Hz clocks beat against that deadline without establishing
a compositor cadence defect.

The Phase 2 plain Wayland EGL client probe (`weston-simple-egl`, with
`WAYLAND_DEBUG=client`) created its first `wl_buffer` through `wl_shm_pool`.
Its rendered pixels survived composition and screencopy, but this was a
`wl_shm` fallback. Client DMA-BUF negotiation remains unproven; this probe
does not establish the GPU-client DMA-BUF path.

Phase 5b publishes the validated headless render node in the kernel registry
as the composite device for backends without a scanout binding. The DMA-BUF
advertiser uses the native GPU binding first, as before, otherwise this node,
otherwise the v3 flat format list (nested). Headless therefore advertises
`zwp_linux_dmabuf_v1` v5-capable feedback with `main_device` equal to the render
node's `dev_t`, using the existing narrowed import format set. Mesa can select its
hardware EGL driver from that feedback without a legacy `wl_drm` global.
The GPU acceptance tests read the default feedback on a real Wayland client,
compare its main device against the selected node's `st_rdev`, and require a
tranche carrying valid format-table indices. The plain EGL client must create
linux-dmabuf buffers and pass the existing rendered-pixel readback checks;
this establishes GPU EGL client allocation, import and composition. The ced
probe continues to record its actual wgpu adapter and attached buffer type.
This does not establish hardware Vulkan WSI on the build workers: their only
Vulkan ICD is llvmpipe and they lack `vulkan-intel`. On a headless desk with
`vulkan-intel` installed, ced chose Vulkan on the Intel GPU and presented
through linux-dmabuf even against the earlier v3 global (2026-10-21,
recorded in the private journal); the Vulkan path depends on the ICD being
in the desk image, which the image builder now includes.

The Phase 5b Intel render-node run observed a version 6 global from the current
Smithay feedback helper. Mesa bound version 4. Default feedback named
`0xe280`, equal to the selected node's (`/dev/dri/renderD128`) `st_rdev`, with
one tranche of 224 format indices.
`weston-simple-egl` created linux-dmabuf buffers; screencopy retained its dark
background and bright bar. Ced chose `Gl`, `IntegratedGpu`,
`Mesa Intel(R) Iris(R) Xe Graphics (RPL-P)`, Mesa 26.2.4, and attached
linux-dmabuf buffers. Idle SSIM was 1.0 and typed SSIM 0.980828380141, with
exact Bus text readback. No console reference was supplied for this run.

Phase 5 uses ced's real egui/wgpu window with a private noded and editd, so
`ced.state {text:true}` can confirm agent-seat typing independently of pixels.
The reference helper `services/compd/crates/testkit/scripts/app_reference.mix`
creates a fresh document and places an 800×560 window at (80,60), scale 1,
in either session; the console-desk reference is captured separately with
that same helper. Before the first GPU run, the acceptance threshold is fixed
at grayscale SSIM ≥ 0.99 for repeated idle captures and the console/headless
comparison, allowing minor rasterisation differences while rejecting material
layout changes. The known typed line must produce SSIM < 0.999 against the
blank document, alongside exact Bus text readback. SSIM uses uniform sliding
8×8 valid windows, population covariance, Rec.601 luma, K1=0.01, K2=0.03 and
L=255. A test-only ced launcher installs a log subscriber around the unchanged
entry point to record the chosen wgpu adapter; Wayland client tracing records
the actual buffer creation request. A shm result is reported as a weaker
client-buffer claim, even when wgpu renders on a GPU. Software wgpu adapters
are recorded explicitly and establish no GPU-client claim.

What the headless-only run proves, and does not (review, 2026-10-21): with no
console reference supplied, the test shows a large, stable rendered change
after typing plus exact model text; it does not check glyph correctness
(tofu or a wrong font fallback would still pass). Glyphs are only pinned by
the console comparison. SSIM at 0.99 tolerates uniform brightness drift: a
+10 luma shift scores about 0.9955 at mid grey and 0.9988 at bright grey, so
a brightness regression is within the threshold by design and must be
caught elsewhere. The tested artefact is the test-only launcher, which
compiles the shipped entry point from the same source; drift is a compile
error, never silent. The first Phase 5 run chose a software Vulkan adapter
(llvmpipe) with `wl_shm`; Phase 5b's device feedback enables the hardware GL
path recorded above. Hardware Vulkan WSI is unproven on the workers and
observed on a desk with the Intel ICD. Open follow-ups
are the test's fixed post-typing sleep (retry once on an unchanged capture),
the helper's leftover `.session` directory on
failure, and its reliance on `spawn` merging the parent environment.

## Wayland conformance (Phase 6)

[canonical/wlcs](https://github.com/canonical/wlcs) exercises protocol behaviour:
buffers, pointer and touch delivery, constraints, clipboard and selection,
subsurfaces, shell roles and popups, output descriptions and supported extensions.
It does not establish KMS, physical input or hardware presentation correctness.
The baseline pins **v1.8.1**, commit
`2b385328366de5db19521b9b3eef81861430d984`.

The private `wlcs` crate produces `libcompd_wlcs.so`. It implements the C ABI
independently, following the structure of
[Smithay 0.7's wlcs_anvil](https://github.com/Smithay/smithay/tree/v0.7.0/wlcs_anvil).
It owns a fresh `compd --headless` child for each server, isolating process-global
configuration, logging and GPU resources across tests. An inherited socket carries
test requests and client descriptors directly to compd's event loop; it has no
listening endpoint. Pointer events call the agent seat's existing policy-host
injection path; touch uses that host's agent hit test, output bounds and native
Smithay touch handles. Window placement calls the same `WindowOp::Place` handler
as `comp.window.place`. Callbacks wait for completion on the compositor loop.
The extension descriptor is read from a live Wayland registry, including the
advertised versions. wlcs requests this before `start`, so that query starts the
child lazily; the later `start` is idempotent. Stop kills and reaps the owned child,
and parent death also kills it after a wlcs abort. The adapter is compiled only
with compd's `wlcs` feature and installed only on the headless backend.

Build on a GPU worker, keeping wlcs outside the MixOS checkout:

```text
git clone --depth 1 --branch v1.8.1 https://github.com/canonical/wlcs /build/wlcs
cmake -S /build/wlcs -B /build/wlcs/build -DCMAKE_BUILD_TYPE=Release \
  -DBOOST_ROOT=/build/wlcs/deps/usr \
  -DCMAKE_CXX_FLAGS=-Wno-error=deprecated-declarations
cmake --build /build/wlcs/build --target wlcs -j4
cargo build -p wlcs --locked
cargo build -p compd --no-default-features --features wlcs --locked
/opt/mixos/bin/mix services/compd/tests/wlcs_gate.mix \
  --wlcs /build/wlcs/build/wlcs \
  --shim /build/mixos/target/debug/libcompd_wlcs.so \
  --compd /build/mixos/target/debug/compd \
  --output-dir /build/wlcs/run-1
```

Wayland development files, gtest/gmock and Boost headers are required. The first
build used Boost 1.92.0 unpacked under `deps`, without a system install. The
compiler flag permits wlcs's deprecated gmock `Invoke` calls with gmock 1.18.0.
`WLCS` may replace `--wlcs`; `COMPD_TEST_RENDER_NODE` selects the render node.

The gate enumerates every test and runs each in its own process, with a 20-second
deadline, preserving JSON, process results and compositor logs in the output
directory. It checks every name and outcome against
`services/compd/tests/wlcs_expectations.conf.mix`; additions, removals and changed
outcomes fail with a diff. The first observation uses `--record --expectations
<new-file>`; recording refuses to overwrite a file. Disabled and unsupported
tests are recorded as skipped. Explicit `flaky` entries still run and must exist,
but their outcomes are excluded from the comparison. Conformance failures are
recorded for later work; Phase 6 does not change compositor policy to fix them.
