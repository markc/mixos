---
title: Command
description: UI-free command registry, typed dispatch and executed journal.
---
<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->

# Command

`command` is the shared, UI-free registry for application engines. It owns
`Command<S>`, `Registry<S>`, `Described`, `Error`, `Scope`, `Origin`,
`Executed`, `Journal` and Fluent `Strings`. Its public contract is 0.3.0.
There are no egui or windowing dependencies.

`Registry::execute(id, state, params, origin)` looks up the command, calls
the optional `authorize` hook, checks enablement, refuses parameters on
nullary commands, then runs the handler and returns its JSON result.
Refusals are `Unknown`, `Denied { id, reason }`, `Disabled { id, reason }`, `BadParams { id, msg }`
or `Failed`. Only successful handlers are journaled. Queries skip the journal;
intent, input and system commands are recorded.

Handlers with parameters declare `ParamSpec { type_name, schema }`, derive
`schemars::JsonSchema` on their serde request struct, and deserialize with
`deny_unknown_fields`, mapping decoding errors to `BadParams`. Schema reporting
does not itself validate a request. Nullary commands accept JSON null or an
empty object; other input is refused. The `nullary!(id, label, handler)` macro
wraps an existing `fn(&mut S)` as an intent returning JSON null.

`describe(state, strings)` keeps id, label, menu, shortcut, more, enabled and
checked. It adds reason (null when enabled), class (`intent`, `query`, `input`,
`system`) and schema (null for nullary commands). Empty `more` is still omitted.
The authorisation hook sees id, origin and params at dispatch; describe reports
state enablement, since it has no caller or parameter body.

Shortcuts and Lucide icon names are strings. Bindings, conflict policy,
overrides, choices, toggles, scopes and changing labels live here. An adapter
supplies chord validation when applying a keymap. `Strings` handles Fluent
catalogues; `Labels` allows an adapter to supply catalogue fallbacks.

[Toolkit](../toolkit/README.md) retains egui shortcut parsing, event handling,
menus and palette rendering. Its existing nullary `Command`, `CommandError`
and three-argument `Registry::execute` remain compatibility wrappers, so
existing application tables need no edits. It re-exports the moved data types.
New engine-owned tables use `toolkit::command::EngineAdapter::new(registry)`:
it caches parsed shortcuts and resolved icons and dispatches typed parameters
through the original registry.

Executed events are **v2**: v, run, seq, at_ms, id, origin, params, result,
then dropped. The accepted params are preserved verbatim, including null;
nullary runs store null params and `note()` stores null for both fields.
The result is the handler's returned value. Each field above 4096 compact
JSON bytes is replaced with `{ bytes, sha256 }`, the byte count and lowercase
SHA-256 of `serde_json::to_vec` output. `executed::bounded` and
`executed::EXECUTED_RESULT_BYTES` expose that policy. `note` and `take_executed`
remain available through toolkit for citizen's publisher, which updates dropped.

`ExecuteRequest { id, params }` is the body type referenced by every
`<app>.execute` row in verb registry schema 4. It derives serde and JsonSchema,
refuses unknown fields, and defaults omitted or null params to None. Existing
apps retain their current parsers during migration.

Authorisation refusal returns Denied before checking enablement, parameters
or running the handler. Toolkit maps it to legacy `CommandError::Denied`.
Both adapters report stored `shortcut` and `more` strings in the keymaps
grammar, in Ctrl, Alt, Shift order. Keys preserve the stored spelling, such as
`ArrowLeft` or `Left`, `=`, `-` and `/`.

The application layering table (`apps/layers.conf.mix`) prevents engine
dependency paths from reaching GUI crates, including through another workspace
library. Unit and integration tests cover dispatch, schema reporting, journals
and the toolkit shortcut behaviour.

## History

`command::History<D>` holds bounded `Arc<D>` snapshots for a cloneable,
comparable document state. `History::new(initial, history::Budget {
max_entries, max_bytes, bytes })` bounds retained undo and redo snapshots;
the current state and saved checkpoint are retained separately. The byte
function measures one snapshot, conservatively counting shared allocations.
Edits clone the current state and commit only on success. Failed edits leave
the state, checkpoint and both stacks untouched. New edits clear redo.

`edit(label, closure)` starts an undo step; consecutive
`edit_coalesced(key, closure)` calls with the same key share a step. An ordinary
edit, undo, redo or `mark_saved()` ends the coalescing group. `current()`,
`can_undo()`, `can_redo()`, `depth()` and `labels()` expose state without mutation.
`mark_saved()` records a checkpoint; `dirty()` compares document values.