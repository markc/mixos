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

- [mixos-mcp](../mixos-mcp/README.md) (cli): MCP stdio access to the native MixOS Bus.
- [monitord](../monitord/README.md) (service): How monitord samples a host's CPU, memory, disks, network and processes, keeps history in memory and SQLite, and answers read commands on the Bus, so any node's load can be read from anywhere on the…

## Server components

- [dnsd](../dnsd/README.md) (service): authoritative mesh DNS for the zones in zones.mix
- [maild](../maild/README.md) (service): JMAP, SMTP, IMAP and DAV mail service
- [toolsd](../toolsd/README.md) (service): Runs the tools webd's admin panel queues (whois, dig, traceroute, shwho) as an unprivileged drain, outside webd's sandbox.
- [webd](../webd/README.md) (service): Virtual hosts, CMS routes, embedded Mix handlers and ACME for the MixOS web service.

## Unclassified

Apps, services and command-line components that do not declare install profiles
yet. They are listed on both this page and the desktop page until they do.

- [backdrop](../backdrop/README.md) (app): The MixOS desktop background — drawn by its own program on the compositor's background layer, on every output, at native and fractional scale.
- [blobd](../blobd/README.md) (service): The node blob store: content-addressed bytes kept in mds, with owner pins, quotas and garbage collection. The MixOS port of blobd, with the store, the Bus citizen, the byte lane and the cross-node…
- [bridged](../bridged/README.md) (service): How bridged puts an external application with a line-delimited JSON control channel on the Bus, one instance per application, and what that exposes.
- [ced](../ced/README.md) (app): The MixOS Editor, a desktop text editor over the edit Bus service, where people and agents edit the same document at once and see each other's carets, selections and changes live.
- [compd](../compd/README.md) (service): The MixOS Wayland compositor, which owns scanout, rendering, input and capture, and serves the comp Bus service with its workspaces, commands, topics and props tree.
- [dopus](../dopus/README.md) (app): MixOS's twin-pane file manager — two folder trees side by side, Places and Properties sidebars, drag and drop between panes, menus and keys for every action, and a Bus surface for agents that reads…
- [editd](../editd/README.md) (service): The edit service, which holds shared text buffers that people and agents edit together, with revision checks, per-origin undo, anchors, atomic save, external-change detection and recovery.
- [email](../email/README.md) (app)
- [hostd](../hostd/README.md) (service): How hostd answers questions about the host and carries out privileged host changes through closed, typed commands behind standing grants, receipts and rollback, so agents never need sudo.
- [inputd](../inputd/README.md) (service): How inputd turns keys into Bus commands below the compositor, keeps the user keys for the person, and holds its keyboard grab only while its own VT is active.
- [mesh-sign](../mesh-sign/README.md) (cli): Signs and verifies the mesh inventory, the trust root of a MixOS mesh.
- [mix-shell](../mix-shell/README.md) (cli): The Mix shell library, holding the interactive line editor, the command classifier, the native-session bootstrap and the Bus citizen runtime that the mix binary runs.
- [mixtrayd](../mixtrayd/README.md) (service)
- [noded](../noded/README.md) (service): The MixOS Bus broker, which handles service registration, discovery, routing, subscriptions, authenticated local session grants and mesh peering between nodes.
- [notifyd](../notifyd/README.md) (service): How notifyd keeps the desk's notifications, shows them as toasts and in the Notifications page, and tells an agent or app which button the human pressed.
- [portald](../portald/README.md) (service): How portald serves the XDG Settings and ScreenCast portals on the session bus, where the appearance values and screen frames come from, and how applications and agents use them.
- [prefs](../prefs/README.md) (app): Appearance, Text & Fonts, Windows, app preferences, Accessibility, and Software & Updates in one window.
- [releasesd](../releasesd/README.md) (service): How releasesd follows upstream application releases on GitHub and installs their Linux builds per user, verified, with rollback, from the command line or the Bus.
- [screenshot](../screenshot/README.md) (app): MixOS's screenshot app — Print Screen for a region, Shift+Print for the screen, Alt+Print for a window; arrows, boxes, pens, highlighter, text, numbered markers, crop and redaction that stay…
- [settingsd](../settingsd/README.md) (service): The headless desktop settings authority, the one writer for a session profile's settings, with durable receipts and retained change publication on the Bus.
- [system](../system/README.md) (cli): The system package builds the mix shell executable with optional noded, monitord, maild, webd and dnsd personalities selected by the name it is started under.
- [term](../term/README.md) (app): The MixOS terminal, with tabs, split panes and a GPU-drawn grid, driven by the session settings and fully scriptable over the Bus.
- [workshop](../workshop/README.md) (app): Workshop is the MixOS desktop app that lists the services and commands on this node's Bus, shows each command's description and the mesh peers, and calls one command once with a JSON body.
