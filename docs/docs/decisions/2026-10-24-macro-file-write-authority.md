---
title: Macro-file write authority
---

# Macro-file write authority

Status: accepted, 2026-10-24.

Macro-file mutations are a deliberate exception to default-open Bus access.
`<app>.macros.put`, `remove`, `move`, `copy` and `new` accept the owning
process's local registered service and Workshop (`workshop` or `workshop-*`).
Global catalogue writes and cross-app moves require Workshop.

Workshop is the trusted catalogue manager. Its mutating macro verbs must
themselves accept only Workshop callers, including requests forwarded to
another app. Its generic execute/call paths must not bypass this restriction.
A person using Workshop's UI retains the manager's authority.

The broker's canonical service name and stamped local origin establish the
caller. Mesh callers cannot mutate macro files. Service registration on a
default-open node is not executable authentication: a local process able to
claim an unused Workshop service name gains that authority. This rule does
not isolate code running under the same filesystem user.

Refusals use `DENIED`, name the verb and rule, and precede file effects.
`list`, `get`, `run`, `status`, `output`, `stop` and `reload` remain open.
Running user code retains its existing authority and is not a sandbox.

Workshop’s Bus `execute` for mutating `macro.*` actions and `bus.call`,
implicit-target `call`, calls forwarding macro mutations or these proxy
operations, and UI click/pointer/key/type injection require Workshop too.
These paths can otherwise turn an untrusted caller into the manager.
Explicit `call` requests for read/run verbs and UI inspection remain open.
