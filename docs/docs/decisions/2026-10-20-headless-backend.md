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

Screencopy and Bus captures run after completed offscreen rendering, including
on idle outputs. The normal picture excludes cursor elements; a second pass
is made only when a capture requests them. Cursor inclusion is chosen per
capture, including mixed requests in one frame, and an unavailable picture
fails its capture rather than substituting another cursor policy. Capturing
unchanged content does not count as a new presentation.
Deployment units, slices and session services are deferred;
this decision introduces no installed headless session.
