# Settings portal contract

Status: accepted, version 0.1.0. The cbc2 verification of `services/portald`
passed on the commit that carries it (dcd2f1a). The verbs and wire shapes below
are fixed by the XDG portal specification.

`portald` implements the Settings interface of the XDG desktop portal, on the
session bus, for the `org.freedesktop.appearance` namespace. The machine-checkable
key table is in [portal.spec.mix](portal.spec.mix).

## Bus identity

| Item | Value |
|---|---|
| Bus name | `org.freedesktop.portal.Desktop` (requested without queueing) |
| Object path | `/org/freedesktop/portal/desktop` |
| Interface | `org.freedesktop.portal.Settings`, version property 2 |
| Error prefix | `org.freedesktop.portal.Error` |

## Methods and signal

- `Read(s namespace, s key) -> v`: the value in one extra variant layer,
  deprecated form.
- `ReadOne(s namespace, s key) -> v`: the value in a single layer.
- `ReadAll(as namespaces) -> a{sa{sv}}`: globs matched against namespaces.
  An unmatched glob yields no namespace entry. An empty array or any empty
  filter matches all namespaces. Up to 64 filters of 256 bytes each are accepted;
  larger requests return `org.freedesktop.portal.Error.InvalidArgument`.
- `SettingChanged(s namespace, s key, v value)`: emitted only when a key's value
  changes.

An unserved namespace or key on `Read` or `ReadOne` returns
`org.freedesktop.portal.Error.NotFound`.

## Keys

| Namespace | Key | Wire type | Values |
|---|---|---|---|
| `org.freedesktop.appearance` | `color-scheme` | `u` | 1 dark, 2 light |
| `org.freedesktop.appearance` | `contrast` | `u` | 0 normal, 1 high |
| `org.freedesktop.appearance` | `accent-color` | `(ddd)` | sRGB 0 to 1; absent when the source has no accent |

Defaults before any projection: `color-scheme` 1 (dark), `contrast` 0, no
`accent-color`.

## Source and identity

- Source verb: `settings.appearance.get`, read-only, from `settingsd`.
- The reply carries the atomic snapshot evidence alongside the projection.
  The shared settings Consumer checks the evidence and manages subscription,
  initial reads, lifecycle generations, delivery loss and bounded recovery.
- Trigger: authenticated retained topic `settingsd.desktop.changed.<profile>`.
  A publication also triggers a read when settingsd restarts at the same revision.
- A projection is identified by binding, incarnation and revision. A projection
  older than the held one within the same incarnation is refused and counted as
  stale. An equal identity changes nothing. A new incarnation is adopted.
- The authority's projection schema is `APPEARANCE_SCHEMA = 1`. An unknown
  schema is refused.

## Startup

1. Load the validated binding-specific cache at
   `<state directory>/<instance>/<profile>/appearance.json`, else defaults.
   A mismatched binding and the old unpartitioned cache are ignored.
2. Request the bus name without queueing immediately. A held name refuses a
   second instance. Authority work runs asynchronously.
3. Answer Read within 250 ms of process launch, tested with the production binary
   and configuration against a silent authority, including cache and profile switch.
4. Serve, and back off on source failure from 250 ms to 30 s. Transport recovery
   remains alive through initial broker failure.

The name is never held waiting on the source.

## Verbs

`portald.status` (read-only) is registered in `docs/spec/bus/verbs.conf.mix`.

## Compatibility

Version 2 of the interface is the one served. A breaking change to a method
shape, key type or key value needs a new interface version, and both versions
must be served during the deprecation period.

## Deferred

- The FileChooser interface.
- Accent as a settings field. The current projection carries the design's
  resolved accent through settingsd's in-memory sidecar, outside the sealed
  snapshot.
- Caller identity and sandboxed app IDs.
