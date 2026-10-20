<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
# command

The UI-free command registry. See the [manual](../../docs/command/README.md)
for dispatch, schemas, classes and toolkit compatibility. Public contract 0.2.0.

The legacy nullary path (`Registry::add_nullary`),
`toolkit::command::{Command, CommandError}`, toolkit's 3-argument
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
