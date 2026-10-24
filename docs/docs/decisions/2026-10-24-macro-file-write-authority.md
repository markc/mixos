---
title: Macro-file write authority
---

# Macro-file write authority

Status: accepted, 2026-10-24.

Macro-file mutations are a deliberate exception to default-open Bus access.
`<app>.macros.put`, `remove`, `move`, `copy` and `new` accept the owning
process's local registered service and Workshop (`workshop` or `workshop-*`).
Global catalogue writes and cross-app moves require Workshop.

Workshop is the trusted catalogue manager. Its mutating macro commands must
themselves accept only Workshop callers, including requests forwarded to
another app. Its generic execute/call paths must not bypass this restriction.
A person using Workshop's UI retains the manager's authority.

The broker's canonical service name and stamped local origin establish the
caller. Mesh callers cannot mutate macro files. Service registration on a
default-open node is not executable authentication: a local process able to
claim an unused Workshop service name gains that authority. This rule does
not isolate code running under the same filesystem user.

Refusals use `DENIED`, name the command and rule, and precede file effects.
`list`, `get`, `run`, `status`, `output`, `stop` and `reload` remain open.
Running user code retains its existing authority and is not a sandbox.

Workshop’s Bus `execute` for mutating `macro.*` actions and `bus.call`,
implicit-target `call`, and calls forwarding macro mutations or these proxy
operations require Workshop too. These paths can otherwise turn an untrusted
caller into the manager. Explicit `call` requests for read/run commands remain
open. Workshop’s UI commands (`workshop.ui.*`) are open to every caller, so an
agent can drive its window: a save made there is Workshop’s own write, as if
a person had pressed Save.
