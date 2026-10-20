---
title: Command
description: UI-free command registry, typed dispatch and executed journal.
---
<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->

# Command

`command` is the shared, UI-free registry for application engines. It owns
`Command<S>`, `Registry<S>`, `Described`, `Error`, `Scope`, `Origin`,
`Executed`, `Journal` and Fluent `Strings`. Its public contract starts at 0.1.0.
There are no egui or windowing dependencies.

`Registry::execute(id, state, params, origin)` looks up the command, calls
the optional `authorize` hook, checks enablement, refuses parameters on
nullary commands, then runs the handler and returns its JSON result.
Refusals are `Unknown`, `Disabled { id, reason }`, `BadParams { id, msg }`
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

Executed events remain **v1**, with exactly the existing body: v, run, seq,
at_ms, id, origin and dropped. `note` and `take_executed` remain available
through toolkit for citizen's publisher.

The application layering table (`apps/layers.conf.mix`) prevents engine
dependency paths from reaching GUI crates, including through another workspace
library. Unit and integration tests cover dispatch, schema reporting, journals
and the toolkit shortcut behaviour.
