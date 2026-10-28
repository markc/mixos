---
title: compd
description: The MixOS Wayland compositor, which owns scanout, rendering, input and capture, and serves the comp Bus service with its workspaces, commands, topics and props tree.
---

# compd

The MixOS Wayland compositor.

Scene model patches and reloads accept every Bus caller. Managed model
patches still require the current nonzero generation. Volume and network
panels accept direct Bus events alongside panel clicks; hostd writes run
locally without a standing grant. Freeze state, write serialisation and
health-check rollback still apply.

compd owns KMS scanout, the GLES renderer and one shared wgpu device, its
own egui surfaces (capture overlays, notifications, the Mix Scenes shell),
the Xwayland window manager, frame pacing, fractional scale, input devices
and capture. On top of that sits the MixOS policy layer: the agent seat,
the `comp` Bus service (commands, topics and the props tree), the frame and
presentation ledgers, panel holders and hot corners, and strict idle
discipline (no frames on a static screen).

Bevy, when enabled, may render effects on the shared wgpu device. It never
owns the frame loop, scheduling, compositing or protocol handling.

## Restoring maximised windows

Dragging a resizable window to an outer side edge previews a half tile; the
top and bottom quarter of each side preview a quarter tile. A 20 logical px
side band takes precedence over the 5 px top band, which previews a real
maximise. Zones use the usable area, so a pointer over an edge panel still
counts. Shared output edges require a 250 ms dwell by default. Shift disables
these zones. A zone change restarts its dwell and show delay.

Release commits only after the shared-edge dwell and preview delay have both
finished. An earlier release leaves the window floating. Escape cancels a move and restores its starting
geometry and placement, including a maximised or tiled start. Dragging away
keeps the original saved floating geometry. Hot corners remain active during
the grab and do not take ownership of a titlebar press.

The preview grows from the window's current frame to the tile over 180 ms,
with cubic ease-out. Retargeting starts from its current animated rectangle.
Disarming, Shift, Escape and grab completion hide it immediately. The preview
uses the theme focus ring: a 25% fill and a solid 2 px
outline, above panels and below screen UI. Tiles advertise all four xdg tiled
edges and square compd's own chrome. Size hints prevent fixed-size tiling and
top maximise when a maximum size cannot fit the usable area.

### Drag tiling props

Read `comp.props.get {path:"tiling"}` and write leaves with
`comp.props.set {path:"tiling.preview_delay_ms",value:100}`. The same props
catalogue describes types, ranges and `persistence:"file"`; props watches
report changes. Valid writes apply live on the next pointer sample and save
the `tiling` object in `preferences.json` beside `settings.json`. The set reply
reports `persisted:true|false`; a failed save leaves the live setting applied.
Older files use defaults; invalid saved leaves individually use defaults.
Corner props remain runtime-only.

| Key (under `tiling`) | Default | Range |
|---|---|---|
| `edge_tiling` | `true` | bool; disables drag zones only |
| `top_maximize` | `true` | bool |
| `corner_ratio` | `0.25` | 0.05–0.5 |
| `side_band_px` | `20` | finite 1–200 logical px |
| `top_band_px` | `5` | finite 1–200 logical px |
| `shared_edge_dwell_ms` | `250` | integer 0–2000 ms |
| `preview_delay_ms` | `0` | integer 0–2000 ms |
| `preview_animation_ms` | `180` | integer 0–1000 ms; zero snaps the preview |

On shared edges the dwell runs first, followed by the preview delay. Both
must finish before release can tile. Outer edges skip the shared-edge dwell,
including the unshared section of a side beside a shorter neighbouring output.
Hot corners keep their independent 4 px detector and remain active in drags.

Timing uses an injectable monotonic policy clock. A calloop one-shot waits
for the pending dwell/delay or the next 16 ms animation step, ending exactly
at the animation deadline. The timer is removed when the grab ends or
disarms. A settled preview and an idle desktop schedule no preview timers
or redraws. Testkit advances frozen policy time without sleeping or rendering.

Docked panels and layer-shell reservations re-fit constrained windows from
their placement and output/workspace split. Saved floating geometry keeps its
size: its centre maps proportionally between usable areas and clamps inside
the new area (oversized rectangles anchor at its top-left). Removing an output
returns its tiled and maximised windows to their saved floating rectangles on
a surviving output, with the same centre mapping. With no output, geometry
waits for a survivor. Workspace moves preserve tile slots and apply the
destination workspace's split. A restore during fullscreen exit-ack applies
geometry and sends its own configure.

Output placement, usable areas, adjacency and drag zones support multiple
outputs; testkit covers two adjacent outputs. Workspace commands and visibility
still follow the default output: independent workspace switching on another
output is refused. Native hotplug maps outputs horizontally in logical Space
coordinates across all spatial worlds. Each monitor views the same world
through its own camera; surface output membership includes every mapped
output, so an explicit placement target or drag pointer selects the monitor.
The native cursor's configured crossing layout is separate from Space's
output geometry. Dormant spatial worlds re-fit when hosted, without admitting
their windows into the active Space. Native keeps a dark primary output anchor
when every monitor disconnects. This tiling support does not make workspaces
independent per monitor.

`comp.window.maximize {id, generation, output?}` accepts an output key or
native output name. Omitting it preserves the existing output choice. An
explicit output retargets an already maximised window directly, without an
intermediate unmaximise, focus, raise or workspace switch. The normal restore
geometry is retained. An unknown output is refused before effects are applied.
This lets toolkit restore a saved maximised frame on its original display.

`comp.window.place_as {id, generation, placement, slot?, output?}` selects
`floating`, `maximized` or `tiled`. Tiled placement requires `slot`: `left`,
`right`, `top`, `bottom`, `top_left`, `top_right`, `bottom_left` or
`bottom_right`. Output accepts its native name or public key. Fullscreen
refuses placement with `invalid_state`; size hints can refuse tiling with
`not_resizable` or maximise with `not_maximizable`. Bad or missing slots
return `invalid_argument`.

`comp.window.tile_key {id, generation, direction}` takes `left`, `right`,
`up` or `down`, through the same operation as Super+arrow. Floating and
maximised windows become the requested half; perpendicular arrows refine a
half into a quarter. Repeating an outward arrow crosses to the adjacent
output and mirrors the slot on that axis. Without an adjacent output it
does nothing. Up from floating selects the top half. Arrows never untile.

Super+PageUp toggles maximise; Super+PageDown minimises. Super+Backspace
restores the focused window: leave fullscreen first, otherwise return tiled
or maximised placement to its saved floating geometry, then raise and focus.
Agents use `comp.window.unfullscreen` or
`comp.window.place_as {id, generation, placement: "floating"}` for those
state changes, followed by `comp.window.focus {id, generation, raise: true}`.
`comp.window.unmaximize` also restores tiled placement to floating.

`comp.window.restore {id, generation, activate?}` only un-minimises: it
preserves fullscreen, placement and saved floating geometry. Activation
defaults to true; `activate:false` preserves focus. Omitting the target, as
Super+Shift+M does, restores the most recently minimised window first.
Minimise alone also preserves placement and floating geometry.

`windows.s<ID>.placement` reports `floating`, `maximized` or the slot name;
`maximized` agrees with it. `float_geometry` is null before the first
constrained placement, then `{x,y,w,h,output}` in global logical content
coordinates, with a public output key. Transitions between constrained
placements preserve that record. `comp.window.placement.changed` publishes
`{id,generation,placement,output,event_seq}` once per actual placement or
target-output change, through the compositor event sequence.

`comp.tiles.get {output?,workspace?}` returns `{output,workspace,split,slots}`:
`split` has `x` and `y` fractions (currently 0.5); each of the eight `slots`
holds an array of `{id,generation}` windows. Defaults are the first output
and its current workspace. Unknown outputs and out-of-range workspaces are
refused. This is a query; placement and keyboard commands are intents.

Nested sessions reserve Right Ctrl+arrow for user bindings. Test keyboard
tiling with a real Super modifier or `comp.window.tile_key`.

## Settings at startup

The session's `appearance.menu_style: "titlebar"` adds a menu caption aligned
left, centre or right, according to `appearance.menu_side`.
It appears only on SSD windows whose `dev.mixos.NAME` app has a registered
`NAME` Bus service. A dotted suffix names an owned viewport:
Email composers (`dev.mixos.email.compose-2`, including the compose-only
root) are excluded: they request client decorations and draw their own one
complete title row, menus and Send control. They never inherit the mailbox's
service-wide menu or action bar, even during decoration negotiation.
Clicking queues `NAME.ui.menu.popup {side, x, viewport?}` through the scene
host's existing port; failures are logged without blocking the compositor.
Titlebar actions carry the same suffix as `window` on `NAME.execute` so the
action reaches the owning viewport. Unsuffixed app IDs keep their root target.
The x coordinate is relative to client content, converted from the logical
decoration slot to the committed client width, without applying output scale
again.
`comp.window.menu {id, generation}` uses the same path, with the usual window
generation and session-lock checks. `comp.props` reports `decoration.menu_button`
(the session's SSD menu request), `decoration.menu_side` and
`decoration.menu_services` (the live names known to chrome).
`surfaces.s<ID>.menu_button` reports the actual window eligibility, using the
same lookup as hit testing and `comp.window.menu`; per-window truth lists a
`menu` button only where it is actually present. Settings and service
registration changes repaint existing chrome live.
The scene host subscribes before reading a full broker registry at startup and
after reconnect. Registry publications identify the broker through either
`from: noded` or its `broker_service: noded` stamp; scene ownership does not
limit the set of app services.

The menu, title, Action Bar and controls use the same title band layout as CSD.
`title_align` and `caption_side` offer style default, left, centre and right;
`actions_align` offers left, centre and right. MixOS titles centre by default.
With `actions_place: "titlebar"`, the scene-host port reads registered,
decorated apps' revisioned `<app>.ui.titlebar.form` properties once on hosting
start, then on trusted `<app>.ui.titlebar.changed` notifications. A 30-second
resync recovers lost notifications. It draws the shared Lucide icons, enabled and toggled states,
and the existing tooltip cards. A click or
`comp.window.action {id, generation, command}` queues `<app>.execute {id}`
through that same port. Disabled and stale pointer actions refuse.
`comp.props` exposes `decoration.actions` and
`surfaces.s<ID>.action_count`. See the
[title band decision](../docs/decisions/2026-10-27-title-band.md).

Quoin prepares cached or embedded settings while its existing Bus
supervisor registers. Its persistent cache is service-owned under the
configured variable data directory at `compd/cache/settings`; all
validation and filesystem work runs on the scene host's Bus worker
(`scene-host` `lane`). `shell.props.get` exposes `settings` and
`settings_cache` evidence. A cache persistence receipt must match the
current applied identity before it describes that presentation. Transport
outages and terminal registration refusal preserve usable appearance. Only
explicit initial refusal permits the configured scene service override; an
established shell identity never switches names. Shutdown drains the newest
activated capture and closes the worker under one two-second budget with a
bounded completion margin. Timeouts report failure without claiming
persistence.

The settings come from [settingsd](../settingsd/README.md) through the
[settings](../settings/README.md) library.

## Toast scenes

A scene with `window: {"kind":"toast","w":360,"h":120}` uses a
separate, pointer-interactive toast seat. It never takes keyboard focus,
including on click or with an authored autofocus field. It draws above
windows and edge panels, below the dialog and session lock.

The draft intent commands are `shell.toast.show {scene, output?, h?}` and
`shell.toast.hide {scene}`. Both reply `{scene, visible, applied:true}`.
Scenes load hidden; show reserves the one toast seat and hide releases it.
A different scene's show returns rc 10 `TOAST_BUSY` with
`holder:{scene, owner}`. Unknown scenes return `NOT_FOUND`, other mount
kinds `NOT_TOAST`, and a nonnumeric height `INVALID_HEIGHT`.

The output defaults to the selected output, as for dialogs. Width clamps
to 240–2048 logical px and height to 60–2048 (defaults 360×120). A show can
override height; showing the same scene again resizes it, and omitting
height preserves its current override. Reload updates the authored geometry
and keeps visibility; changing kind requires an unload first.

Placement is top right, 12 logical px inside the usable area left by
layer-shell exclusive zones and docked scene panels. The drawn size fits
inside that area on small outputs. Chrome defaults to false; `chrome:true`
adds a title frame and a close button that hides the toast.

## Layout

| Path | What |
|---|---|
| `src/` | the `compd` binary: CLI, event loop, Bus and scene wiring |
| `crates/` | compd's private crates (engine, protocols, policy, backends) |
| `layers.conf.mix` | the private crates' dependency order, checked by `tools/layering_gate.mix` |
| `tests/` | integration tests |

The source is
[`services/compd`](https://github.com/markc/mixos/tree/main/services/compd).
The desktop-tier gates that run compd nested or on hardware live in
[`tests/desktop/`](https://github.com/markc/mixos/tree/main/tests/desktop)
at the repository root.

## Build

```
cargo build --profile release-fast -p compd                     # nested backend
cargo build --profile release-fast -p compd --no-default-features \
    --features backend-native                                   # KMS backend
cargo build --profile release-fast -p compd --features backend-all   # all three
cargo build --profile release-fast -p compd --no-default-features \
    --features backend-headless                                 # headless
```

`capture-ffmpeg` adds hardware video capture through libav; the default
build links no FFmpeg.

`desktop-dbus` explicitly enables the optional desktop compatibility
adapters. The default native session has no D-Bus client or portal
dependency. It passes environment directly to children; delegated cgroup
placement before exec and `cgroup.kill` provide complete cleanup when
`MIXOS_CONTAIN_CHILDREN=1`.

## Run

Nested, inside another Wayland session:

```
compd --nested --socket wayland-compd --config-file=settings.json
```

On a VT, through the seat (libseat/seatd):

```
compd kms-live --device /dev/dri/card1 --connector HDMI-A-1 --scale 2.5
```

compd waits for its own VT to become active and never switches VT itself.
`--bus-service NAME` and `--scene-service NAME` override the Bus names
(default `comp`); `compd --help` lists every flag. Settings are read from
`--config-file`, else `~/.config/compd/settings.json`. Preferences are read
from `preferences.json` beside it.

Native fixed-refresh outputs use late-latch composition by default. After a
flip with pending redraws, each CRTC waits until the next kernel-timestamped
vblank minus its render budget. Two 50-commit buckets retain the maximum time
from the armed deadline (or clamped plan instant) to commit return, including
dispatch delay and sibling rendering. Exportable render fences also contribute
their kernel signal timestamp through a non-blocking calloop source. GPU
callback dispatch latency is excluded; unavailable or invalid timestamps leave
GPU tails to the learned floor. Modesets and the first commit and flip after reset do not
train steady-state cost or phase.

Deadline anchoring accepts DRM scanout timestamps up to 2 ms ahead of the
handler's monotonic clock, covering an interrupt delivered during blanking
before active scanout begins. Larger future offsets and dispatch delays over
200 ms are rejected. The tearing phase keeps its existing arrival fallback
for future timestamps.

The budget is the larger of that measured maximum and a learned floor, plus
`late_latch_margin_us` (default 1500). A miss on a deferred frame raises the floor
to the submitted budget minus margin plus one eighth of a refresh, capped at
one refresh interval. The floor shrinks by only 50 µs per 100 target hits;
idle gaps preserve that run. A miss, or unknown feedback on a deferred frame,
breaks the run. The initial estimate is 2 ms. The 1.5 ms margin covers residual jitter
and GPU observation error; hardware timing still needs measurement under load.

Set `late_latch:false` in the settings file to disable it, or set the emergency
environment switch `COMPD_DISABLE_LATE_LATCH` (any value). Both take effect at
startup without rebuilding; the margin uses the same settings file. Schema 7
adds these fields to older files in memory. Startup, the first redraw after
idle, untrustworthy timestamps, over-budget frames, tearing, governed pacing,
active VRR on that CRTC and active capture retain immediate composition.
The VRR preference alone does not disable late latch: unsupported connectors,
refused requests and VRR-off pipes still use fixed-refresh deadlines. The gate
reads cached committed DRM state, independently for each output.
Nested and headless
backends keep their existing pacing. Frame callbacks and presentation feedback
use their existing paths, including empty-frame callback timers.

With `COMPD_FRAME_TRACE=1`, `kms_latch_deferred` and `kms_latch_deadline`
describe the wait and render; other `kms_latch_*` decision stages name the
immediate-path reason. `kms_latch_hit`, `kms_latch_miss` or `kms_latch_unknown`
on completion records the result. These ledger trace events use the same
output ID as `kms_queue_frame`, a target CLOCK_MONOTONIC timestamp in `detail`
(microseconds, zero if unavailable), and the budget in `aux` (microseconds).
The result uses the submitted frame's budget. A hit allows half an interval of
timestamp tolerance; a full missed vblank is a miss. Tracing off performs no
extra ID hashing or trace clock reads.

With the `backend-headless` feature, a render node can drive a virtual
output without a VT, seat, input device or parent compositor:

```
compd --headless --socket wayland-headless --size 1920x1080 --scale 1 \
    --render-node /dev/dri/renderD128 --config-file settings.json
```

The output is `headless-0` at 60 Hz; `--size` sets physical pixels and
`--scale` sets the logical-to-physical scale (defaults 1920x1080 and 1).
`--render-node` defaults to the `render_node` setting. EGL must report the
selected node. The default Bus name is `comp-headless`, overridden with
`--bus-service`; the binding profile is `headless`, without VT or host
interception chords. XWayland is disabled. Startup preserves inherited
scheduling and nice values and does not publish desktop session variables.

Frames repaint into alternating offscreen textures only when the scene
changes or a capture is due, capped at 60 Hz. `COMPD_HEADLESS_REFRESH_HZ`
selects 30–60 Hz (default 60) for both the virtual mode and timer.
Completion is waited on before screencopy, frame callbacks and software
presentation feedback. Both `zwlr_screencopy` and `comp.capture.frame` work
on idle outputs. Captures choose cursor inclusion individually: the normal
frame excludes cursor elements, and a second pass runs only for captures
requesting them. Callback-only commits and unchanged captures do not
increase the window's `presentation.presented` counter.

The ignored `headless` GPU tests need a built `compd` with this feature
beside the test binaries, or selected by `COMPD_TEST_BINARY`. The capture
tests also need built `noded` and `testkit-screenshot` binaries beside them;
they start an owned private broker, without using a running desktop's Bus.
The plain EGL client probe needs `weston-simple-egl` (override its path with
`COMPD_TEST_EGL_CLIENT`); it records the first buffer's actual creation
request and checks captured pixels.

The Phase 5 app test also needs `editd`, the testkit binaries (including
`testkit-ssim`) and `cargo build -p ced --example ced-headless-probe`.
This test-only launcher adds wgpu adapter logging to ced's normal entry point;
production apps and backends keep their behaviour. The test runs ced against
its own Bus and compositor, checks an idle pair at SSIM ≥ 0.99, types through
the agent seat, requires SSIM < 0.999 and reads the exact text through
`ced.state`. PNGs and the client trace remain in `headless-app-artifacts`
beside the built binaries. Supply `COMPD_TEST_APP_REFERENCE=/absolute/ref.png`
to additionally compare the initial headless window with a console reference
at the fixed 0.99 threshold; absence is explicitly reported as unmeasured.

Capture that reference in the console session environment with a running
`editd`, no existing ced window and a scale-1 output:

```text
<prefix>/bin/mix services/compd/crates/testkit/scripts/app_reference.mix /tmp/ced-reference.png --comp comp --output OUTPUT
```

The same helper launches the headless test window. It gives ced fresh private
app state, places its geometry at (80,60), waits for 800×560, creates a blank
document and captures its window without the cursor via `comp.capture.frame`.
Its last stdout line is JSON containing the PNG path, requested geometry,
observed window row, scale and capture metadata. It closes its own child after
capture; the GPU test uses `--keep` to take ownership for input checks. Use the
same theme, fonts and application build for both references. Session artefacts
are left beside the PNG in `<path>.session`, which must not already exist.
`testkit-ssim A.png B.png --threshold 0.99` compares the pair; it also accepts
`--crop X,Y,WxH`, or tightly packed raw RGBA with `--size WxH`. Its grayscale
metric uses uniform sliding 8×8 valid windows, population covariance, rounded
Rec.601 luma, K1=0.01, K2=0.03 and L=255; alpha is ignored. It returns 1 below
threshold and 2 for invalid arguments or images, including mismatched sizes.

`compd-headless@.service` runs inside a headless desk with its own noded,
session bus, PipeWire and portald. `%i` is the session slot (for example
`rec`), with settings in `<config>/session/%i.env` and Bus service `comp`
inside that desk. It uses the render group, no seatd or tty, nice 10 and
`mixos-headless.slice` (`CPUQuota=200%`). Provisioning places the desk's
capture services and encoder in the same slice. Both units are owned by
`services/compd/units/`; they do not create the desk.

`testkit-presentation-probe --rate 60 --hold-s 60` measures independent
commit pacing and presentation misses. Add `--stall-every 30 --stall-ms 50`
for a positive control. Its header documents the pixel frame ID marker;
`services/compd/crates/testkit/scripts/frame_ids.mix FILE.rgba --size WxH`
decodes unscaled raw RGBA client content (PNG is not supported).
Run timing assertions with an optimised compositor, built with
`--profile release-fast` and selected through `COMPD_TEST_BINARY`; the test
harness and probe can remain debug builds. Report the binary profile with
the measured rates and misses. Debug compositor overhead is part of its
measured presentation latency and can dominate a one-interval deadline.

`cursor_sensitivity` in `preferences.json` is a speed multiplier on top of
the output scale: compd multiplies each relative pointer delta by both, so
`1.0` moves the cursor at the same logical speed at every scale. Clients
asking for relative motion get the raw deltas.

## Session activity

`comp.session.get {}` is a draft query returning
`{"version":1,"active":true,"vt":3,"since_ms":1792108800123}`. `active`
describes this compositor's libseat session, including while paused after a
console VT switch. `since_ms` is Unix milliseconds at the last transition,
or at compd's Bus startup before the first transition.

Topic `<service>.session.changed` (normally `comp.session.changed`) publishes
`{"version":1,"active":false,"vt":3,"since_ms":1792108800456,"event_seq":42}`
once at startup and once on each pause or resume. It uses the shared
observation sequence, outbox and gap reporting. The publisher runs on its
own Bus worker, so pause events do not wait for rendering or resumption.
Subscribe before querying to establish the current state without polling;
on an observation gap, query again. Across desks, the most recent
`active:true` transition identifies the desk the human is viewing.

The session unit supplies `MIXOS_VT=%i`. compd reads it once when starting
its Bus service; `vt` is a u32 or `null` when the variable is absent or
cannot be parsed. A nested compositor with no supplied VT reports `null`.
The VT is never inferred from a runtime path or display name.

## Workspaces

Every output has a current workspace; windows belong to one. Commands on the
`comp` service:

| Command | Arguments | Reply |
|---|---|---|
| `comp.workspaces.list` | none | `{count, current, o_<key>: {current}, list: [{index, windows, name}]}`, the same object as `comp.props.get {path: "workspaces"}`; `name` is null when unnamed |
| `comp.workspace.switch` | `{index, output?, wrap?}` | `{output, from, to}` |
| `comp.window.send_to_workspace` | `{id, generation, index, follow?}` | the window |
| `comp.workspaces.set_count` | `{count: 1..=16, persist?}` | `{from, count, persisted?, persist_error?}` |
| `comp.workspace.rename` | `{index, name, persist?}` | `{index, name, previous, persisted?}` |

`index` is 1-based; switch and send also take `"next"` and `"prev"`.
Shrinking the count moves every window above it to the last workspace and
drops the removed workspaces' names. A name is trimmed, at most 64
characters, with no control characters; `""` clears it. `persist` defaults
to true: the count and the names are written to `preferences.json`
(`workspaces_count`, `workspace_names`) and compd starts with them. If the
file cannot be written, the change still applies and the reply has
`persisted: false` and `persist_error`. A session lock refuses every
workspace change and hides the names (and window counts) from readers.

Topic `comp.workspace.changed` (`<service>.workspace.changed`) carries
`{version: 1, count, current, outputs: {o_<key>: current}, names, event_seq}`
whenever the count, a current workspace or a name changes. `names` has one
entry per workspace, null when unnamed. Window counts are not in it;
`comp.props.changed` on `workspaces.list` carries them. The two topics are
sequenced separately, so a subscriber may see one before the other.

## Test

```
cargo test --workspace
mix tests/desktop/nested_smoke.mix --help
mix tests/desktop/parity_suite.mix --help
```

Logging: `--debug[=LEVEL]` and live Bus controls are described in
[live application logging](../docs/dev/debugging.md).
