---
title: Decision — commands take parameters and return results
description: Typed parameters, returned results and one UI-free command registry for every dispatch path.
---
<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->

# Decision — commands take parameters and return results

Status: accepted 2026-10-20. Amends [egui applications](2026-10-09-egui-applications.md), [executed commands](2026-10-11-executed-commands.md) and [command classes](2026-10-11-verb-classes.md).

Amended 2026-10-24 by [Bus commands and app actions](2026-10-24-bus-commands-and-actions.md): the names below are now command, command class, command family and action.

## The decision

- **A command has typed parameters and a result.** Its parameter type is a serde struct that refuses unknown fields and derives a JSON Schema; `<app>.actions` reports the schema, enablement and the disabled reason; `<app>.execute { id, params }` runs it and returns the result.
- **The registry is UI-free** (`libs/command`). Engine crates own their command tables; the egui crate renders menus, shortcuts and the palette from them.
- **One path.** UI, shortcuts, palette, macros, Bus and headless all dispatch through the registry. A command that performs an action a command defines is refused in review.
- **Executed events carry `params` and `result`** (`v: 2`); a value above 4 KiB is replaced by `{ bytes, sha256 }`.
- **Every registered command names its body schema** (registry schema 4): a request type, `none`, or `pending` through 2026-11-30 inclusive in UTC.

## Why

An action's id alone cannot express the choices needed to run it again.
Typed parameters make those choices explicit, and a result lets every caller
observe the same outcome. One registry keeps validation and enablement
consistent across the UI, Bus and headless paths.

## Consequences

- Engines own serde request types and their schemas; handlers deserialize
  accepted parameters and return results.
- Events preserve the accepted JSON body verbatim, rather than reserialising
  handler defaults. Nullary runs carry null parameters; `note()` carries null
  parameters and results.
- Each parameter and result value is measured separately using compact JSON
  bytes. Above 4096 bytes it becomes `{ bytes, sha256 }`, with a lowercase
  SHA-256 digest of those bytes. This bounds the executed journal.
- `<app>.actions` reports stored chords in Ctrl, Alt, Shift modifier order,
  preserving key names from the keymaps grammar, including short forms.
- Command owners replace migration placeholders with request types or `none`
  by the cutoff. The gate refuses expired placeholders by command or family name.
