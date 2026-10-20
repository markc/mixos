<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
# command

The UI-free command registry. See the [manual](../../docs/command/README.md)
for dispatch, schemas, classes and toolkit compatibility. Public contract 0.1.0.

The legacy nullary path (`Registry::add_nullary`),
`toolkit::command::{Command, CommandError}`, toolkit's 3-argument
`Registry::execute`, and the `Engine*` aliases are transitional and will be
removed in plan step 7.

Core `describe()` emits the stored chord grammar (`Ctrl+=`, `ArrowLeft`);
the toolkit wrapper retains egui NAMES spelling for today's consumers, and
plan step 2 switches `<app>.commands` to core spelling as part of the
describe/schema contract change.

An authorize refusal surfaces as `Disabled` today; a distinct `Denied`
error comes with the executed v2 body in plan step 2.
