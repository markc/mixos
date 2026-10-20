# Portal contract: Settings and ScreenCast

Settings status: accepted, version 0.1.0 (ScreenCast is a draft, below). The cbc2 verification of `services/portald`
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

`portald.status` (read-only) is registered in `docs/docs/spec/bus/verbs.conf.mix`.

## Compatibility

Version 2 of the interface is the one served. A breaking change to a method
shape, key type or key value needs a new interface version, and both versions
must be served during the deprecation period.

## ScreenCast

Status: draft. Served with `portald serve --screencast`; first verified on
desk12 VT4, 2026-10-20 (a 3840x2160 frame received through the portal by a
PipeWire consumer). The method and signal shapes are fixed by the XDG portal
specification; what MixOS offers within them is below.

| Item | Value |
|---|---|
| Interface | `org.freedesktop.portal.ScreenCast`, version property 5, same object |
| `AvailableSourceTypes` | 1 (monitor) |
| `AvailableCursorModes` | 3 (hidden, embedded) |
| Request path | `/org/freedesktop/portal/desktop/request/<sender>/<handle_token>` |
| Session path | `/org/freedesktop/portal/desktop/session/<sender>/<session_handle_token>` |

`<sender>` is the caller's unique name without the leading `:` and with `.`
as `_`. A token is 1 to 64 ASCII letters, digits or `_`; an absent token is
generated.

- `CreateSession(a{sv}) -> o`: `Response` 0 with `session_handle` (`s`).
- `SelectSources(o, a{sv}) -> o`: `types` must include 1; `cursor_mode` 1 or 2
  (default 1); otherwise `InvalidArgument` at the call. `multiple` and
  `persist_mode` are accepted and ignored. `Response` 0.
- `Start(o, s, a{sv}) -> o`: `InvalidArgument` before `SelectSources` or when
  already started. `Response` 0 with `streams` `a(ua{sv})` (one stream:
  `position (ii)`, `size (ii)` in logical coordinates, `source_type` 1, `id`
  the output name) and `persist_mode` 0; `Response` 2 when nothing can be
  captured.
- `OpenPipeWireRemote(o, a{sv}) -> h`: after a successful `Start` only.
- Every `Response` is addressed to the owner. `Request.Close` by the owner
  removes the request with no `Response`; a `Start` closed this way stops its
  cast.
- A session is the caller's alone: another caller gets `NotFound` (methods)
  or `AccessDenied` (`Session.Close`). It ends on `Session.Close` (no
  signal), on its owner leaving the bus, or with `Session.Closed` when capture
  stops on its own.
- The stream is a PipeWire node with `media.class` `Video/Source`, `BGRx`, the
  output's mode size, variable rate up to 30 fps, memfd buffers.

Consent: every `Start` is granted by the `allow` policy, a standing grant
matching the caller's process name, or an answer to a notifyd question (Allow /
Deny, 60 s, timeout denies). A refusal answers `Response` 1. Bus verbs, all
local-only (`broker_origin` must be `local`, else rc 13 `FORBIDDEN`):
`portald.screencast.pending` (query), `portald.screencast.answer {request,
allow}` (input), `portald.screencast.grant {app?, count?, ttl_ms?}` (intent),
`portald.screencast.revoke {id?}` (intent). Registered in
`docs/docs/spec/bus/verbs.conf.mix`.

## Deferred

- The FileChooser interface.
- ScreenCast: window sources, several streams, restore
  tokens, DMA-BUF frames and a permission-limited remote.
- Accent as a settings field. The current projection carries the design's
  resolved accent through settingsd's in-memory sidecar, outside the sealed
  snapshot.
- Caller identity and sandboxed app IDs.
