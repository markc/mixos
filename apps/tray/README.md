# tray

`tray` is the MixOS tray item. It is a StatusNotifierItem, so it sits in
Plasma's own tray host and needs no panel of its own. Its tooltip gives a
count of active MixOS units, and its menu lists each watched unit with its
systemd state.

It is read-only. Starting, stopping and log actions are not in this version.

## Behaviour

- Registers as `dev.mixos.tray` with `assume_sni_available`, so starting it
  before the tray host is ready does not fail.
- Reads unit states over D-Bus, from `org.freedesktop.systemd1`, when the
  menu opens. There is no timer and no polling loop.
- Watches `hostd.service`, `monitord.service` and `webd.service` on the system
  manager. The templated units `maild@.service` and `noded@.service` need an
  instance name, so they are not watched yet.

## Build and test

Rust builds run on the cbc workers, not on the desk:

```
mix _bin/cbc-build.mix --worker cbc2 --repo mixos -- cargo test -p tray
```

## Not yet

- The `dnsd`, `indexd` and `blobd` services are named in the port plan, but
  they have no unit in this tree yet, so they are not listed.
- Session units for a MixOS desk (`session@N.target` and its user units) need
  the desk's user manager, which this version does not query.
- No autostart entry is installed. The session wiring decides how it starts.
