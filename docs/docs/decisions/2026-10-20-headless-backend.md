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
independently, with the callback structure informed by
[Smithay 0.7's wlcs_anvil](https://github.com/Smithay/smithay/tree/v0.7.0/wlcs_anvil).
Small client/surface lookup fragments in the compd adapter follow that reference.
The C ABI shim and owned-child lifecycle were implemented for compd. The `/256`
conversion implements Wayland's signed 24.8 fixed-point ABI; `touch_destroy`
auto-up is local lifetime handling (wlcs_anvil's touch destroy is empty).
Both source headers and `NOTICE` credit the reference; its MIT licence remains
in `vendor/smithay/LICENSE.txt`.
It owns a fresh `compd --headless` child for each server, isolating process-global
configuration, logging and GPU resources across tests. An inherited socket carries
test requests and client descriptors directly to compd's event loop. That control
socket has no listening endpoint; the child also listens on `--socket wayland-wlcs`
inside its private runtime directory. Pointer events call the agent seat's existing
policy-host injection path; touch uses that host's agent hit test on down and
Smithay's held focus for motion and up. Refused delivery returns an explicit error
on the control socket and is printed with the compositor log path. Window placement
uses the native policy-host placement executor with a test-only entry point:
it retains redraw, pointer retargeting and interactive-grab completion while
permitting deliberately off-output positions. All new
entry points are behind `wlcs`. Callbacks wait for completion on the compositor loop.
The extension descriptor is read from a live Wayland registry, including the
advertised versions. Creation probes a short-lived child and reaps it to cache
the descriptor before wlcs asks for it. The descriptor callback only reads the
cache, including after stop; only explicit start retains a test compositor.
Stop kills and reaps the owned child,
and parent death also kills it after a wlcs abort. The adapter is compiled only
with compd's `wlcs` feature and installed only on the headless backend.

Placement uses the requested **wl_surface origin** at screen `(x, y)`. The shim
converts through the native camera transform (world `(0, 0)` is screen centre),
adds the committed xdg geometry offset to obtain the Space element location,
then reads the public window row and checks that its surface origin projects
back to `(x, y)`. Successful checks appear in the gate output. Absolute pointer
and touch coordinates use the same conversion, avoiding a half-screen shift.

There is a contract discrepancy in the pinned wlcs tests. `display_server.h`
describes the window's left/top; both `pointer_respects_window_geom_offset` and
`touch_respects_window_geom_offset` in `tests/xdg_toplevel_stable.cpp:131,162`
(and their v6 equivalents at `103,134`) expect local input coordinates
`input - placement + geometry_offset`, which positions the **window geometry**
origin. They use offset `(35, 12)` and input delta `(20, 30)`, expecting
`(55, 42)`. Under the explicitly selected surface-origin convention, the correct
local point would be `(20, 30)`. Smithay's reference adapter also uses the geometry
origin: `vendor/smithay/wlcs_anvil/src/main_loop.rs:213` maps the element at the
wlcs location without adding the geometry offset. The pinned tests and that
reference agree; this adapter's selected surface-origin convention differs.
This contract mismatch is established from source, not inferred from a failed
run. No test source or normal compositor behaviour is changed to conceal it.

The fresh recording reports the four named cases as follows (coordinates below
are pixels, converted from Wayland 24.8 values):

| Case | Observed result |
| --- | --- |
| `XdgToplevelStableTest.pointer_respects_window_geom_offset` | Fails the coordinate assertion: `(2.5, 24)` observed, `(55, 42)` expected by wlcs. Placement self-check reports `(200, 280)`. |
| `XdgToplevelStableTest.touch_respects_window_geom_offset` | `touch_down` refuses `touch_no_target`; the touched-window assertion sees NULL, before the coordinate assertion. Placement self-check reports `(200, 280)`. |
| `XdgToplevelV6Test.pointer_respects_window_geom_offset` | Skipped: missing `zxdg_shell_v6 >= 1`. |
| `XdgToplevelV6Test.touch_respects_window_geom_offset` | Skipped: missing `zxdg_shell_v6 >= 1`. |

The stable pointer result also differs from `(20, 30)`, so the convention mismatch
alone does not explain that failure. The native coordinate/focus path still needs
tracing; the stable touch case stops at native hit testing. Neither stable case
reports `unconfigured_buffer` in this recording, and neither v6 case tests input.

Build on a GPU worker, keeping wlcs outside the MixOS checkout:

```text
git clone --depth 1 --branch v1.8.1 https://github.com/canonical/wlcs /build/wlcs
cmake -S /build/wlcs -B /build/wlcs/build -DCMAKE_BUILD_TYPE=Release \
  -DBOOST_ROOT=/build/wlcs/deps/usr \
  -DCMAKE_CXX_FLAGS=-Wno-error=deprecated-declarations
cmake --build /build/wlcs/build --target wlcs -j4
cargo build -p wlcs --locked
cargo build -p compd --no-default-features --features wlcs --locked
cargo clippy -p wlcs --all-targets --no-deps --locked -- -D warnings
cargo --config 'env.COMPD_WLCS_BINARY="/build/mixos/target/debug/compd"' \
  test -p wlcs --locked -- --ignored --nocapture
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
The ignored shim lifecycle test needs an absolute `COMPD_WLCS_BINARY` because
Cargo runs it from the package directory. It checks cached descriptors before
start and after stop, including stable backing storage. `--no-deps` scopes
Clippy's warning gate to the shim rather than inherited legacy dependency lints.

The gate enumerates every test and runs four isolated processes at a time, each with a 20-second
deadline, preserving JSON, process results and compositor logs in the output
directory. It checks every name and outcome against
`services/compd/tests/wlcs_expectations.conf.mix`; additions, removals and changed
outcomes fail with a diff. The first observation uses `--record --expectations
<new-file>`; recording refuses to overwrite a file. Disabled and unsupported
tests are recorded as skipped. The `flaky` list must be empty: exemptions are
refused, and every observed outcome must match. The control socket's 10-second
read/write timeout is half the process deadline; transport errors name the
compositor log before aborting. Conformance failures are
recorded for later work; Phase 6 does not change compositor policy to fix them.

The first complete observation (2026-10-21) recorded **1,228 tests: 361 passed,
563 failed, 304 skipped, 0 classified flaky**. Every failed case produced a
complete JSON report; no case hit the gate's process deadline. Unsupported
protocols use wlcs's `wlcs-skip-test` JSON property, even though gtest also
records an exception for them. These are skips, alongside disabled cases.

After the shim review, a fresh recording from `ab43adf4` on 2026-10-21 produced
the same inventory and outcomes. The expectation file was removed before
recording; its data was regenerated rather than preserving any exemptions.

| Outcome or failure class | Before review | Fresh recording |
| --- | ---: | ---: |
| Passed | 361 | 361 |
| Failed | 563 | 563 |
| Skipped | 304 | 304 |
| Flaky | 0 | 0 |
| `xdg_surface` 3, `unconfigured_buffer` | 319 | 319 |
| `zwlr_layer_surface_v1` 0, `invalid_surface_state` | 79 | 79 |
| Input-region assertions | 58 | 58 |

Every one of the 1,228 cases produced complete JSON; none hit a process deadline,
terminated by signal or reported an adapter error. **237 cases passed the
surface-origin self-check.** Unchanged outcomes do not validate the old shim:
the named stable `touch_and_drag_on_surface_seen` and
`touch_drag_outside_of_surface_and_back_not_lost` cases still fail with
`unconfigured_buffer` before reaching input injection, so they cannot prove
touch continuity until the runtime configure defect is fixed.

Two subsequent complete gate runs against committed baseline `de856d52` passed
the name-by-name outcome comparison. Each again recorded **361 passed, 563 failed,
304 skipped, 0 flaky**, with 237 successful placement checks and complete JSON
for all 1,228 cases. Neither run hit a process deadline, terminated by signal or
reported an adapter error. The expectation data and both observed inventories
were also compared structurally and matched exactly.

The original three largest classes were xdg unconfigured-buffer errors (319),
layer-surface invalid-state errors (79), and input-region assertions (58).
The remaining original classes were 38 event timeouts, 24 missing-notification
cases, 7 keyboard-focus cases and 38 other assertions or protocol errors.
The interface name is part of every protocol error's identity:

- **`xdg_surface`, error 3 = `unconfigured_buffer`.** Stable
  [xdg-shell.xml](https://gitlab.freedesktop.org/wayland/wayland-protocols/-/blob/main/stable/xdg-shell/xdg-shell.xml)
  starts this enum at 1: `not_constructed`, `already_constructed`,
  `unconfigured_buffer`, `invalid_serial`, `invalid_size`. Compd emits error 3
  on correctly configured wlcs surfaces, for example
  `ForeignToplevelManagerTest.detects_toplevel_closed`, yet fails to emit it in
  `XdgSurfaceStableTest.attaching_buffer_to_unconfigured_xdg_surface_is_an_error`
  (the report says the expected error was not received). This is a **compd
  configure-state defect** for a later phase; its root cause has not been traced.
  Start at `vendor/smithay/src/wayland/shell/xdg/mod.rs:1673` (toplevel
  pre-commit check, posted at `1721`), and `2067` (popup, posted at `2116`).
  The hooks are installed by `xdg/handlers/surface.rs:130,208`.
- **`zwlr_layer_surface_v1`, error 0 = `invalid_surface_state`.** The 79-case
  baseline class is on this interface, not `zwlr_layer_shell_v1`:
  `Layer/LayerSurfaceLayerTest.surface_on_lower_layer_is_initially_placed_below/0`
  reports `must ack the initial configure before attaching buffer`. This is
  another **compd configure-state defect**, raised in
  `vendor/smithay/src/wayland/shell/wlr_layer/mod.rs:576`.
  [Layer-shell XML](https://gitlab.freedesktop.org/wlroots/wlr-protocols/-/blob/master/unstable/wlr-layer-shell-unstable-v1.xml)
  defines **`zwlr_layer_shell_v1`, error 0 = `role`** separately: another role
  is already assigned. That check is at
  `vendor/smithay/src/wayland/shell/wlr_layer/handlers.rs:84,85`, with the
  role stamp in `wayland/compositor/tree.rs:170`. An erroneous early or stale
  assignment on a fresh surface would be a compd defect, but the original logs
  contain
  **zero** failures with that interface/error pair. Calling the observed
  79 failures role errors would misattribute the protocol object.
- **Input-region targeting and coordinates:** the shim's surface placement
  and screen/world conversion are corrected above. Remaining input-region
  assertions exercise compd's native hit testing and are **compd defects**;
  for example, `MultiRectEdges/RegionSurfaceInputCombinations.input_inside_region_seen/8`
  verifies surface origin `[64, 49]`, completes absolute motion, then reports
  `no_pointer_target` on the button operation. Its paired touch case reports
  `touch_no_target` on down. These are native hit misses after verified placement,
  not silently successful adapter deliveries. No protocol error code is raised.
  Start at `services/compd/crates/world/src/surface/interface/core/hit.rs:478`
  (fitted surface coordinates) and `vendor/smithay/src/desktop/wayland/window.rs`
  (`surface_under`). The stable geometry-offset cases have the observed native
  coordinate/hit failures recorded above, in addition to the source-level
  **wlcs/shim placement-contract mismatch**.
- **Event timeouts, missing notifications and keyboard focus:** these are
  **compd conformance gaps** in the native event, constraints, layout and focus
  paths. They have no protocol error code unless the individual report names
  one. In particular, pointer constraints miss expected locked/unlocked events;
  exclusive-zone layout can time out. The adapter's device refusals are explicit
  in the case logs and must be considered when tracing each case.
- **Other assertions or protocol errors:** retained per case, including
  missing mandated errors for invalid buffers and unconfigured surfaces,
  popup placement/lifecycle and output metadata assertions. These are compd
  conformance defects; the geometry-offset cases also have the stated contract
  mismatch, which does not fully explain their observed failures.
  They are not evidence of a protocol-permitted refusal of a valid client.

All failed cases remain in the expectation file; this phase fixes the shim and
attributes runtime conformance work rather than changing normal compd policy.
