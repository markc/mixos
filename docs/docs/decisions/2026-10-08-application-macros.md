---
title: Every application supports Mix macros
description: Every native MixOS app lists and runs ordinary Mix scripts as macros, from built-in, user-global and user per-app folders.
---

# Every application supports Mix macros

Status: accepted, 2026-10-08. Storage amended 2026-10-23: three tiers, with
a user's own macros under `~/.config/mixos/macros`.

Every native MixOS GUI application has macros by default. A macro is an
ordinary Mix program (`.mix`); there is no separate workflow language or
broker. Macros give people and agents one programming surface across the
desktop: a script can drive any app's commands and Bus verbs, and reach
other nodes over ABP between noded instances. Macros run with the normal
session authority of the user who runs them.

## Where macros live

| Tier | Where | Written by |
|---|---|---|
| Built-in global | `/opt/mixos/share/macros/*.mix` | releases, read-only |
| Built-in per-app | `/opt/mixos/share/macros/<app>/*.mix` | releases, read-only |
| User global | `$XDG_CONFIG_HOME/mixos/macros/*.mix` (default `~/.config/mixos/macros/`) | the user; every app lists these |
| User per-app | `$XDG_CONFIG_HOME/mixos/macros/<app>/*.mix` | the user; only that app lists these |

- `<app>` is the app's component name. A file at the top of a macros
  folder is global; a folder holds one app's macros.
- A macro's id is its file stem. When two tiers have the same id, user
  per-app beats user global, which beats built-in. The catalogue shows each
  macro's tier and path, and marks one that shadows another.
- Listing never runs a macro. Only `.mix` files are macros; `.mx`
  documents are refused.
- New and edited macros are written only to the user tiers. Editing a
  built-in macro copies it to the user tier first.
- `/opt/mixos` holds only shipped files. A shared desk sees its host user's
  macros through the shared home; an isolated desk's user keeps their own.

## Where macros appear

Every app's icon bar has the standard Macros icon. It opens the app's
macros: run, edit, new, open folder and reload, with the running macro's
output and a stop action. The shared discovery, runner and panel live in
shared libraries; apps supply only their command registration and any
domain context.

## Not yet decided

The Bus verbs for agents (`<app>.macros.list`, `run`, `status`, `stop`,
`output`, `new`, `reload`), folder watching and per-app context adapters
are planned work, not part of this statement.
