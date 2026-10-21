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
client-buffer claim, even when wgpu renders on a GPU.
