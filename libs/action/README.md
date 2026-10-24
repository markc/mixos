<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
# command

The UI-free command registry. See the [manual](../../docs/action/README.md)
for dispatch, schemas, classes and toolkit compatibility. Public contract 0.3.0.

The legacy nullary path (`Registry::add_nullary`),
`toolkit::action::{Command, CommandError}`, toolkit's 3-argument
`Registry::execute`, and the `Engine*` aliases are transitional and will be
removed in plan step 7.

Core and toolkit `describe()` emit stored chords in the keymaps grammar
(`Ctrl+=`, `ArrowLeft`, or the app's short forms), ordered Ctrl, Alt, Shift.

An authorize refusal returns `Denied` before enablement or parameter checks.
A Bus-level `DENIED` test waits for the first app to set `authorize` (dopus, plan step 5).

`ExecuteRequest` names the strict serde body for `<app>.execute`. Executed
events are v2, with accepted params and returned result, each bounded to 4096
compact JSON bytes inline; larger values carry `{ bytes, sha256 }` instead.

Plan step 6's registry conformance test will check that each `<app>.execute`
row's schema matches the app's real parser.

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

## Follow-ups

Screenshot's `restyle` still changes ink and width on the selection directly,
without journaling a document edit. An `edit.restyle {id, colour, width}`
command belongs in plan step 5/6.

The annotation command table classifies document errors by message text.
Replace that classification with a typed document error.
