# portald

The MixOS Settings portal. It serves `org.freedesktop.portal.Desktop` on the
session bus, object `/org/freedesktop/portal/desktop`, interface
`org.freedesktop.portal.Settings` version 2, and answers the
`org.freedesktop.appearance` namespace from the settings authority (settingsd).
The manual is in [docs/portal.md](../../docs/portal.md); the contract is in
[docs/spec/portal](../../docs/spec/portal/README.md).

```text
portald serve --instance example [--profile default] [--state-dir DIR]
```

It needs `DBUS_SESSION_BUS_ADDRESS`. `--state-dir` defaults to
`$STATE_DIRECTORY`, which systemd sets from `StateDirectory=portald`.

## Behaviour

- Startup requests the bus name immediately with cached or default values.
  Authority work is asynchronous and never delays name acquisition. The
  production binary must answer Read within 250 ms of process launch.
  An existing owner is refused, not queued.
- Values come from one `settings.appearance.get` read, triggered by the
  retained `settingsd.desktop.changed.<profile>` topic. The shared settings
  Consumer handles initial connection, subscription, generation fencing, loss
  and recovery. The appearance reply includes its atomic snapshot evidence;
  the projection must match its binding and identity.
- A projection is adopted only when it is not older than the held one within
  the same incarnation. Replacement is atomic.
- `SettingChanged` is emitted for changed keys only.
- A failed read backs off, from 250 ms doubling to 30 s, and clears on success.
  The last-good values keep being served meanwhile.
- Transport recovery continues indefinitely through initial broker failure.
- ReadAll accepts at most 64 filters of 256 bytes each, with iterative bounded
  matching. An empty array or any empty filter selects all namespaces.
- A validated cache at `$STATE_DIRECTORY/<instance>/<profile>/appearance.json`
  seeds cold start. Foreign bindings are rejected. The old unpartitioned cache
  is ignored. Cache data is never written back to settingsd.

## Verbs

| Verb | Read only | Reply |
|---|---|---|
| `HELP` | yes | the verb manifest |
| `portald.status` | yes | origin, name ownership, identity, last fault, counters, settings connection generation |

## Tests

```text
cargo test --locked -p portald
```

Unit tests cover value mapping, iterative matching and acceptance.
`tests/portal.rs` runs a private `dbus-daemon` and checks version 2, Read and
ReadOne nesting, empty and adversarial ReadAll filters, NotFound, changed-only
signals, cache isolation and single ownership. It launches the production
binary against a silent authority and requires a complete Read reply within
250 ms, including cached startup and profile switching.

Native tests embed the real noded via `libs/test-broker` and run the production
settingsd loop. They cover initial fetch, live update without reconnect,
settingsd restart at unchanged revision, and broker recovery after the initial
attempt budget is exhausted. These tests run by default; the build cluster
needs `dbus-daemon` and the pinned Rust dependencies.

## Deferred

- FileChooser (`org.freedesktop.portal.FileChooser`) and its chooser app.
- The accent is the private settingsd stub. The design does not yet export a
  first-class accent field.
- Sandboxed callers and the session D-Bus activation file are not wired here.
