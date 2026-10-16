---
title: compd
description: The MixOS Wayland compositor, which owns scanout, rendering, input and capture, and serves the comp Bus service with its workspaces, verbs, topics and props tree.
---

# compd

The MixOS Wayland compositor.

compd owns KMS scanout, the GLES renderer and one shared wgpu device, its
own egui surfaces (capture overlays, notifications, the Mix Scenes shell),
the Xwayland window manager, frame pacing, fractional scale, input devices
and capture. On top of that sits the MixOS policy layer: the agent seat,
the `comp` Bus service (verbs, topics and the props tree), the frame and
presentation ledgers, panel holders and hot corners, and strict idle
discipline (no frames on a static screen).

Bevy, when enabled, may render effects on the shared wgpu device. It never
owns the frame loop, scheduling, compositing or protocol handling.

## Settings at startup

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
cargo build --profile release-fast -p compd --features backend-all   # both
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

`cursor_sensitivity` in `preferences.json` is a speed multiplier on top of
the output scale: compd multiplies each relative pointer delta by both, so
`1.0` moves the cursor at the same logical speed at every scale. Clients
asking for relative motion get the raw deltas.

## Workspaces

Every output has a current workspace; windows belong to one. Verbs on the
`comp` service:

| Verb | Arguments | Reply |
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
