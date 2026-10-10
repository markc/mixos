---
title: The Settings portal
description: How portald serves the XDG Settings portal on the session bus, where the appearance values come from, and how applications and agents read them.
---

# The Settings portal

Applications on MixOS ask the desktop for its appearance (dark or light mode,
contrast and accent colour) through the XDG Settings portal. `portald` is the
MixOS implementation of that portal. It owns `org.freedesktop.portal.Desktop` on
the session bus and answers the `org.freedesktop.appearance` namespace.

Values come from the settings authority, `settingsd`. The portal never writes
settings. Before authority or cache data is available, it serves the documented
defaults.

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
`accent`. A projection is refused when it is older than the held one within the
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
it keeps `DBUS_SESSION_BUS_ADDRESS`.

```text
portald serve --instance <name> [--profile default] [--state-dir DIR]
```

## Status for agents

The bus verb `portald.status` is read-only. It reports the origin of the held
values (defaults, cache or settingsd), whether the name is owned, the identity
of the held projection, the last fault and the counters. Its
`settings_generation` reports the Consumer's connection generation, or null
while disconnected. It is registered in
`docs/spec/bus/verbs.conf.mix`.

## Not yet

- The FileChooser portal and its chooser application.
- A first-class accent field. Accent currently comes from the design's resolved
  accent, held in settingsd outside the sealed snapshot, and will move into the
  settings contract when that is decided.
- Sandboxed callers and D-Bus activation.

The contract, with its status, is in [docs/spec/portal](spec/portal/README.md).
