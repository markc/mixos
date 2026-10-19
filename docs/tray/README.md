---
title: Tray
description: The MixOS tray item, a StatusNotifierItem on Plasma's own tray host that shows the state of MixOS units.
---

# Tray

`tray` is the MixOS tray item (app id `dev.mixos.tray`). It registers as a
StatusNotifierItem, so it sits in Plasma's tray host and needs no panel of
its own. The tooltip gives a count of active MixOS units, and the menu lists
each watched unit with its systemd state.

It is read-only in this version. Starting, stopping and log actions are not
yet in it.

The source is [`apps/tray`](https://github.com/markc/mixos/tree/main/apps/tray).

## Behaviour

- Registers with `assume_sni_available`, so starting it before the tray host
  is ready does not fail.
- Reads unit states over D-Bus from `org.freedesktop.systemd1` each time the
  menu opens. There is no timer and no polling loop.
- Watches `hostd.service`, `monitord.service` and `webd.service` on the
  system manager. The templated units `maild@.service` and `noded@.service`
  need an instance name, so they are not watched yet.

## Not yet

- `dnsd`, `indexd` and `blobd` have no unit in this tree, so they are not
  listed.
- Session units for a MixOS desk need that desk's user manager, which this
  version does not query.
- No autostart entry is installed. The session wiring decides how it starts.
