---
title: Server components
description: The MixOS components in the server install profile, generated from their declared profiles.
generated: by docs/_build/gen-site.mix from tools/component_index.mix --json; do not edit
---

# Server components

The components a MixOS server installs, generated from each component's declared
install profiles. A component in more than one profile is listed on each page. Every
component, with its kind and source, is in the [component index](components.md); see also
the [desktop](desktop.md) list. This page describes the current `main` branch of MixOS.

## Core components

On every MixOS system.

None yet.

## Server components

None yet.

## Unclassified

Apps, services and command-line components that do not declare install profiles
yet. They are listed on both this page and the desktop page until they do.

- [backdrop](../backdrop/README.md) (app): The MixOS desktop background — drawn by its own program on the compositor's background layer, on every output, at native and fractional scale.
- [bridged](../bridged/README.md) (service): How bridged puts an external application with a line-delimited JSON control channel on the Bus, one instance per application, and what that exposes.
- busviewer (app; no page yet)
- ced (app; no page yet)
- compd (service; no page yet)
- [dopus](../dopus/README.md) (app): MixOS's twin-pane file manager — two folder trees side by side, Places and Properties sidebars, drag and drop between panes, menus and keys for every action, and a Bus surface for agents that reads…
- [editd](../editd/README.md) (service)
- [hostd](../hostd/README.md) (service): How hostd answers questions about the host and carries out privileged host changes through closed, typed verbs behind standing grants, receipts and rollback, so agents never need sudo.
- mix-shell (cli; no page yet)
- [noded](../noded/README.md) (service)
- [portald](../portald/README.md) (service): How portald serves the XDG Settings portal on the session bus, where the appearance values come from, and how applications and agents read them.
- [prefs](../prefs/README.md) (app): MixOS's preferences window — a sidebar of editors: Applications follows, installs, updates, rolls back and removes upstream releases; Appearance sets the session's colour scheme, style, mode,…
- [releasesd](../releasesd/README.md) (service): How releasesd follows upstream application releases on GitHub and installs their Linux builds per user, verified, with rollback, from the command line or the Bus.
- [screenshot](../screenshot/README.md) (app): MixOS's screenshot app — Print Screen for a region, Shift+Print for the screen, Alt+Print for a window; arrows, boxes, pens, highlighter, text, numbered markers, crop and redaction that stay…
- settingsd (service; no page yet)
- system (cli; no page yet)
- term (app; no page yet)
