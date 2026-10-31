---
title: The desktop portal
description: How portald serves XDG Settings, FileChooser v4 and ScreenCast on the session bus, and how applications and agents use them.
---

# The desktop portal

Applications on MixOS ask the desktop for its appearance (dark or light mode,
contrast and accent colour) through the XDG Settings portal, and for a screen
to share through the XDG ScreenCast portal. `portald` is the MixOS
implementation of these interfaces and FileChooser version 4. It owns `org.freedesktop.portal.Desktop` on the session
bus, answers the `org.freedesktop.appearance` namespace and, when started with
`--screencast`, serves monitor sources from the compositor.

Values come from the settings authority, `settingsd`. Before authority or
cache data is available, the portal serves the documented defaults. portald
writes no files for other toolkits: the GTK and Qt files are
[exportd-gtkqt](../exportd/gtkqt.md)'s.

## Where the values come from

```text
settingsd  --(retained topics: settingsd.desktop.keys.<profile>.appearance|window)-->  portald
portald    --settings.get {values} + settings.palette (bounded reads)----------->  settingsd
```

portald follows the initial connection and later generations, subscribes, and
reads the resolved mode and `appearance.export_toolkits` with `settings.get`
(`values`) and the
contrast and accent with `settings.palette`; the two answers must carry the
same binding, incarnation and revision. Authenticated
retained deliveries trigger fresh reads, including after settingsd restarts.

A projection (schema 4) carries the binding (instance and profile), an
incarnation, a revision, `mode`, `contrast`, an sRGB `accent` and `export`,
the value of `appearance.export_toolkits`. A cache of an older schema is
refused and the portal waits for settingsd. With `export` false the portal
states no preference: `color-scheme` 0, `contrast` 0 and no `accent-color`,
so portal clients fall back to their own defaults. A projection is refused when it is
older than the held one within the
same incarnation. A new incarnation is adopted. A repeat of the held identity
changes nothing and emits no signal.

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

## File chooser

`org.freedesktop.portal.FileChooser`, version 4, is served on the same Desktop
object by default. `OpenFile`, `SaveFile` and `SaveFiles` return a Request
path and forward to `files.choose`, served by `dopus --choose-service` on
the native Bus. rfd and other portal clients find the interface through the
Desktop object's D-Bus introspection and `version` property.

Filters, a standalone current filter, choices and byte-array paths follow the
[files contract](../docs/spec/files/README.md). Unknown options are ignored;
validly typed options are adjusted to the native limits, with each adjustment
logged. A known option with the wrong type, an invalid NUL terminator or
SaveFiles without usable names returns `InvalidArgument` before exporting a
Request. Paths become file URIs without a UTF-8 conversion. `on_behalf_of`
names the D-Bus peer and its process as a claim, never as authority.

Responses go only to the owner: 0 with URIs and applicable choices/filter,
1 for person cancellation, 2 for refusal or failure. Owner Close and owner
disconnect cancel the chooser without a Response. Non-owner Close is
`org.freedesktop.portal.Error.NotAllowed`. An early outcome waits for its
matching native request id. A native reply taking over five seconds fails
the Request; a late chooser is cancelled. Reused handle tokens cannot consume
an old chooser's outcome.

The bridge subscribes before forwarding and resubscribes on native Bus
reconnect. While Requests remain, it reads `files.choose.list` every five
seconds. A vanished id is recovered through `files.choose.result`; an unknown
id, list failure or changed incarnation ends the affected Request with 2.
Startup and reconnect cancel old portal-correlated choosers before admitting
new ones. This temporary polling is removed when noded has lifecycle topics.

portald owns the portal frontend name itself. It is not an
`org.freedesktop.impl.portal.*` backend for a second xdg-desktop-portal
frontend, and it does not provide sandbox document export or access grants.

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
of the held projection, the last fault and the counters. Its
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
FileChooser mapping tests cover each option, non-UTF-8 byte paths, filters,
choices and lenient limits. Lifecycle tests permute reply, outcome, Close,
disconnect, provider loss and timeout, checking at most one Response and
none after Close. `tests/filechooser.rs` runs a private session D-Bus, real
noded and a scripted files provider with a persistent zbus caller. It checks
all three methods, early outcomes, refusals, concurrent callers, ownership,
Close during send/open, token reuse, missed outcomes, timeout and provider loss.
The same test checks startup cleanup and native Bus loss/reconnect.

`services/portald/tests/chooser_bus_test.mix` is a separate CBC acceptance
test requiring Weston. It starts a private session D-Bus, noded, headless
Weston, nested compd, portald and `dopus --choose-service`. Its persistent
zbus probe drives each method's real viewport through `files.ui.*`, including
save replacement and cancellation. The probe also opens, saves and cancels
through rfd 0.17.2 (a dev dependency), with external fallbacks unavailable.
Build on CBC with `cargo build -p portald -p dopus -p compd -p noded -p system --bins --examples`,
then run `MIXOS_TEST_BIN=target/debug mix services/portald/tests/chooser_bus_test.mix`.
It is excluded from `tools/mix_tests.mix`, which needs no headless host.
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

- A first-class accent field. Accent currently comes from the design's resolved
  accent, held in settingsd outside the sealed snapshot, and will move into the
  settings contract when that is decided.
- Sandboxed callers and D-Bus activation.
- For ScreenCast: window sources, several streams per
  session, restore tokens, DMA-BUF frames and a permission-limited PipeWire
  remote.

The contract, with its status, is in [docs/docs/spec/portal](../docs/spec/portal/README.md).
