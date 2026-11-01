# Explicit global declarations

Status: accepted. Decision: Mark, 2026-11-01. Language: Mix 0.119.0.

Function assignment defaults to a local binding while reads fall through to
the program-global frame. A helper doing `$conns = delete($conns, $key)`
therefore reads the global map but stores a local copy. This has caused
repeated connection leaks. Handler bodies and helpers previously behaved
differently for the same assignment.

## Decision

Add the reserved statement `global $a, $b`. Once executed, assignments to
these names, including self-concat, compound assignments, in-place mutations
and index/field/path writes, target the program-global frame through the
same shared-global machinery used by serve handlers. Assignment creates a
missing global; a missing read remains an undefined-variable error.

Declarations belong to the current function invocation, including recursive
calls independently. They are not inherited by called functions or lambdas.
Lambdas must declare too: one rule applies to all function invocations.
Declaration inside control flow is allowed and idempotent; it takes effect
only when reached. Lint checks textual order without evaluating conditions,
and notes conditional declarations.

Reject a parameter (`GLOBAL_PARAM`), a name already in the current local
frame, including captures and module injections (`GLOBAL_AFTER_LOCAL`), a
declared global used as a loop/catch/parse binder (`GLOBAL_BINDER`), and the
runtime-owned names `$event`, `$rc`, `$result`, `$reply`
(`GLOBAL_RUNTIME_NAME`). Module-variable errors explain that module variables
are not globals. A same-named local and declaration cannot coexist.

At ordinary program or module top level the declaration is a no-op by
construction. In a handler it is honoured: in Class C, an otherwise new name
can be created in the invocation's local root and disappear; declaring it
routes creation to the shared program-global frame. Lint emits a note outside
functions. A declaration inside a required module function targets the
importer's program globals, never the module's captured state.

`global` cannot name a function. Map keys and fields remain valid:
`{global: 1}`, `$m.global`, `$m["global"]`.

## Implementation constraints

Keep declaration sets in Scope alongside function floors. Exclude declarations
from synchronous function eligibility and local-slot caching: a helper that
creates a global must never rehash the global map while a top-level loop
holds raw slot pointers into it. Shared globals retain their existing
statement-sized borrow discipline; declaration adds no atomic transaction
across yields.

## Alternatives rejected

Write-through by default breaks helpers that deliberately reuse names such as
`$r` and `$m` as locals: the existing fleet agent lint found 463 outer-name
assignment hits. Keep local assignment as the default.

Lint alone protects against mistakes but leaves no direct way to express an
intentional program-global update. Path writes into a shared map remain
useful, but are not a substitute for an explicit declaration.

The earlier no-global-by-design rule is superseded by this decision.
