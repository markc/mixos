---
title: The desktop portal
description: How portald serves the XDG Settings and ScreenCast portals on the session bus, where the appearance values and screen frames come from, and how applications and agents use them.
---

# The desktop portal

Applications on MixOS ask the desktop for its appearance (dark or light mode,
contrast and accent colour) through the XDG Settings portal, and for a screen
to share through the XDG ScreenCast portal. `portald` is the MixOS
implementation of both. It owns `org.freedesktop.portal.Desktop` on the session
bus, answers the `org.freedesktop.appearance` namespace and, when started with
`--screencast`, serves monitor sources from the compositor.

Values come from the settings authority, `settingsd`. Portald exports derived
GTK caption settings into the desk's private config directory. Before authority
or cache data is available, the portal serves the documented defaults.

## Where the values come from

```text
settingsd  --(retained topic: settingsd.desktop.changed.<profile>)-->  portald
portald    --settings.appearance.get (one bounded read)------------->  settingsd
```

The shared settings Consumer processes the initial connection and later
generations, subscribes, and reads the projection with `settings.appearance.get`.
The reply includes its atomic snapshot evidence. The Consumer checks that
evidence, and the projection must match its binding and identity. Authenticated
retained deliveries trigger fresh reads, including after settingsd restarts.

A projection carries the schema version, the binding (instance and profile), an
incarnation, a revision, the design revision, `mode`, `contrast` and an sRGB
`accent`. The settingsd wire view remains schema 1 for older strict readers.
After checking that view against its atomic snapshot, portald derives a schema 2
projection with `caption_side` from the snapshot's effective desktop context,
without another settingsd read. Its cache retains that caption placement;
older schema 1 caches default to `right`. A projection is refused when it is
older than the held one within the
same incarnation. A new incarnation is adopted. A repeat of the held identity
changes nothing and emits no signal.

## GTK caption settings

At startup from a validated cache, and on every accepted projection, portald
merges these keys into `[Settings]` in `$XDG_CONFIG_HOME/gtk-3.0/settings.ini`:

| Key | Value |
|---|---|
| `gtk-decoration-layout` | `menu:minimize,maximize,close` for right captions; `close,minimize,maximize:menu` for left captions |
| `gtk-icon-theme-name` | `mixos` (constant; there is no icon-theme setting) |

Craft apps use this file on the `MixOS` desktop with `CRAFT_TITLEBAR=app`. They
resolve `window-minimize-symbolic`, `window-maximize-symbolic`,
`window-restore-symbolic` and `window-close-symbolic` through the MixOS icon
theme in `$XDG_DATA_DIRS/icons`. The original 16-unit hairline SVGs install from
`share/icons` into `<prefix>/share/icons/mixos/scalable/actions`.

The export runs only when `XDG_CONFIG_HOME` is set, absolute and different from
`$HOME/.config`; otherwise it logs once at info level and skips. Parent segments
and symlinks in the destination path are also refused. This keeps a nested desk
from overwriting its host desktop's settings.

Other lines, comments and sections are preserved byte for byte. Unchanged
content is left alone. Changed content is written to a sibling temporary file,
synced and renamed, then the directory is synced. The directory is created if
needed. Reads are capped at 64 KiB; larger, non-regular or unreadable files are
logged and left alone. An export failure never stops the portal. Without a
validated cache, exporting waits for settingsd's first accepted projection.

## What applications see

Interface `org.freedesktop.portal.Settings`, version 2, at
`/org/freedesktop/portal/desktop` on `org.freedesktop.portal.Desktop`.

| Key | Type | Values |
|---|---|---|
| `color-scheme` | `u` | 1 dark, 2 light |
| `contrast` | `u` | 0 normal, 1 high |
| `accent-color` | `(ddd)` | sRGB, each component 0 to 1 |

Methods:

- `ReadOne(namespace, key)` returns the value with one variant layer.
- `Read(namespace, key)` returns the value wrapped in one extra layer. This is
  the deprecated form, kept for older clients.
- `ReadAll(globs)` returns `a{sa{sv}}`, matched against namespace globs.
  An empty array or any empty filter returns all namespaces. At most 64 filters
  of 256 bytes each are accepted; larger requests return
  `org.freedesktop.portal.Error.InvalidArgument`. Matching uses bounded iteration.

An unserved key returns `org.freedesktop.portal.Error.NotFound`. The
`accent-color` key is absent until a projection with an accent has been adopted.
`SettingChanged(namespace, key, value)` is emitted only for keys whose value
actually changed.

Example, from a shell with `busctl`:

```sh
busctl --user call org.freedesktop.portal.Desktop /org/freedesktop/portal/desktop \
  org.freedesktop.portal.Settings ReadOne ss org.freedesktop.appearance color-scheme
```

## Screen sharing

Interface `org.freedesktop.portal.ScreenCast`, version 5, on the same object.
Video calls and browsers use it to share the screen: Zoom, Firefox and
Chromium `getDisplayMedia()`, OBS.

```text
compd   --zwlr_screencopy_manager_v1 (shm, damage copies)-->  portald
portald --PipeWire Video/Source node (BGRx, default ceiling 30 fps)--> the caller
```

| Property | Value |
|---|---|
| `AvailableSourceTypes` | 1: monitors only |
| `AvailableCursorModes` | 1 hidden, 2 embedded |
| `version` | 5 |

A caller runs `CreateSession`, `SelectSources`, `Start`, then
`OpenPipeWireRemote`. Each of the first three returns a request path at once and
answers later with a `Response` signal addressed to the caller, as the portal
specification describes. Request and session paths are derived from the
caller's unique name and its `handle_token` / `session_handle_token`, so a
caller can subscribe before it calls. `Start` answers with one stream:

| Key | Meaning |
|---|---|
| node id | the PipeWire node to connect to |
| `position`, `size` | the output in compositor (logical) coordinates |
| `source_type` | 1, monitor |
| `id` | the output name, such as `HDMI-A-1` |

Frames are captured at the output's mode size with `copy_with_damage`, so a
static screen costs nothing. The newest frame is sent again after a second
without a new one, and as soon as a consumer attaches. The node is a
`Video/Source` named `portald-screencast`, which is what the session manager
links capture streams to.

Sessions belong to the connection that created them. Another caller cannot
select, start or close them (`NotFound`, `AccessDenied`). A session ends when
its owner calls `Session.Close`, when the owner leaves the bus, or when capture
stops on its own (then `Closed` is emitted). `Start` answers 2 when there is
nothing to capture, for example no compositor or no output with a mode.

### Consent

Nothing is captured until the request is granted, in this order:

1. **Policy.** `--screencast-consent allow` grants everything. Use it only for
   headless and test sessions. The default is `prompt`.
2. **A standing grant** that covers the caller's app (its process name, as in
   `/proc/<pid>/comm`, so at most 15 bytes). This is how automation shares
   the screen with nobody at the desk. A process name says which app is
   asking but does not prove it, since any local process can set its own: a
   named grant is a convenience for a trusted, unsandboxed session, not an
   access control. A caller whose name cannot be read is covered only by a
   grant for any app.
3. **The person.** portald posts a notifyd question, "Share your screen?", with
   Allow and Deny. The request waits as a pending prompt until the button is
   pressed, an agent answers it over the Bus, or 60 seconds pass (up to 65
   when the question could not be posted). A timeout, Deny
   or a closed question all answer the app with 1 (cancelled). If the app
   closes the request or its session, or leaves the bus, while it waits, the
   question is taken down and nothing is captured. At most 4 requests wait at
   once, one per caller; more are denied straight away. Without notifyd the
   request still waits, and can only be answered over the Bus.

| Command | Body | Does |
|---|---|---|
| `portald.screencast.pending` | `{}` | lists waiting requests (`request`, `app`, `owner`, `output`, `notification`), the grants, and the policy |
| `portald.screencast.answer` | `{request, allow}` | decides a waiting request, as if the person pressed the button; withdraws the question |
| `portald.screencast.grant` | `{app?, count?, ttl_ms?}` | covers the next `count` (default 1, at most 100) Starts by `app` (any app when absent) for `ttl_ms` (default 5 minutes, at most an hour); at most 16 grants |
| `portald.screencast.revoke` | `{id?}` | withdraws one grant, or all of them |

These commands accept every broker-stamped caller, local or mesh.
Missing provenance is a malformed request (rc 10). notifyd likewise accepts
answers and withdrawals from any node, so a remote agent can press Allow.
Every grant, answer and decision is logged with the caller.

Consent prevents accidental sharing. Any caller on the Bus can answer or
grant access; it is not an access boundary between mesh nodes or processes.

To share the screen from a script with nobody at the desk:

```text
send portald portald.screencast.grant body='{"app":"obs","count":1,"ttl_ms":60000}'
```

Then start the share in the app. To drive the prompt instead, read
`portald.screencast.pending` and answer the request it lists.

This is a first cut, scoped to the trusted, unsandboxed session:

- **One stream per session:** the first output with a current mode.
  `multiple` is accepted and ignored.
- **No window sources and no restore tokens.** `persist_mode` is accepted and
  ignored.
- **shm frames,** copied twice per frame. DMA-BUF is later work.
- **The remote is unrestricted.** `OpenPipeWireRemote` returns a plain
  connection to the session's PipeWire, not one limited to the cast node.

## Startup and failure

- Startup takes the bus name immediately with a valid cache for the requested
  binding, or defaults (dark, normal contrast, no accent). Authority reads run
  asynchronously and never delay name acquisition. A production process test
  requires a complete Read reply within 250 ms of launch, with a silent authority.
- The name is requested without queueing. If another portal already owns it,
  this instance exits. It does not wait in line.
- A failed read backs off from 250 ms, doubling to a 30 s ceiling, and resets on
  success. The last-good values keep being served while the authority is down.
- The supervised transport keeps retrying through initial broker failure.
- The cache is `<instance>/<profile>/appearance.json` under the state directory,
  capped at 64 KiB and written by atomic rename. Mismatched bindings are rejected;
  the old unpartitioned cache is ignored. It seeds the next cold start and is
  never sent back to settingsd.

## Running it

The unit is `portald@.service`, owned by `services/portald/units/`. It is part of
`session@<instance>.target` and wants `noded@<instance>` and
`settingsd@<instance>`. It runs as `mixos` with the session environment file, so
it keeps `DBUS_SESSION_BUS_ADDRESS`. The unit passes `--screencast`.

```text
portald serve --instance <name> [--profile default] [--state-dir DIR] [--screencast]
    [--screencast-consent prompt|allow] [--screencast-max-framerate 30]
```

It needs `DBUS_SESSION_BUS_ADDRESS`. `--state-dir` defaults to
`$STATE_DIRECTORY`, which systemd sets from `StateDirectory=portald`. With
`--screencast` it also needs `WAYLAND_DISPLAY` and `XDG_RUNTIME_DIR` for the
compositor, and `PIPEWIRE_RUNTIME_DIR` / `PIPEWIRE_REMOTE` (or
`XDG_RUNTIME_DIR`) for PipeWire. The binary links the system `libpipewire-0.3`.

`--screencast-max-framerate N` sets the frame-rate ceiling offered to cast
consumers, from 1 through 120 Hz; invalid values are refused with
`SCREENCAST_MAX_FRAMERATE`. The default stays 30. A headless desk may pass 60
in a unit drop-in; its own PipeWire and compositor refresh must support it.

## Status for agents

`HELP` returns the command manifest. The bus command `portald.status` is read-only. It reports the origin of the held
values (defaults, cache or settingsd), whether the name is owned, the identity
of the held projection, the last fault and the counters. `gtk_settings` reports
the last export's path, outcome (`written`, `unchanged` or `skipped`) and reason.
Its
`settings_generation` reports the Consumer's connection generation, or null
while disconnected. It is registered in
`docs/docs/spec/bus/commands.conf.mix`.

## Tests

The source is
[`services/portald`](https://github.com/markc/mixos/tree/main/services/portald).

```text
cargo test --locked -p portald
```

Unit tests cover value mapping, iterative matching and acceptance.
`tests/portal.rs` runs a private `dbus-daemon` and checks version 2, Read
and ReadOne nesting, empty and adversarial ReadAll filters, NotFound,
changed-only signals, cache isolation and single ownership. It launches the
production binary against a silent authority and requires a complete Read
reply within 250 ms, including cached startup and profile switching.

`tests/screencast.rs` runs the ScreenCast contract on a private `dbus-daemon`
with no compositor: properties, the request and session paths, the Responses,
refused source types and cursor modes, ownership, `Start` answering 2 with
nothing to capture, `Close`, and sessions retired when their owner leaves.
Capture itself needs compd and PipeWire. On a desktop session,
`cargo run -p portald --example screencast_probe` drives one session and holds
it so a consumer can attach, for example
`gst-launch-1.0 pipewiresrc target-object=<serial> ! videoconvert ! autovideosink`.

Native tests embed the real noded via [test-broker](../test-broker/README.md)
and run the production settingsd loop. They cover initial fetch, live
update without reconnect, settingsd restart at unchanged revision, and
broker recovery after the initial attempt budget is exhausted. These tests
run by default; the build cluster needs `dbus-daemon` and the pinned Rust
dependencies.

## Not yet

- The FileChooser portal and its chooser application.
- A first-class accent field. Accent currently comes from the design's resolved
  accent, held in settingsd outside the sealed snapshot, and will move into the
  settings contract when that is decided.
- Sandboxed callers and D-Bus activation.
- For ScreenCast: window sources, several streams per
  session, restore tokens, DMA-BUF frames and a permission-limited PipeWire
  remote.

The contract, with its status, is in [docs/docs/spec/portal](../docs/spec/portal/README.md).
