---
title: Decision — Mix components
description: A component may be written entirely in Mix. Its identity lives in a component.mx document instead of a Cargo package, and the component gates read both.
---

# Decision — Mix components

Status: accepted 2026-10-10. Amends [the source layout](2026-10-04-source-layout.md)
(AGENTS.md §2 and §10).

## The decision

- **A component may be written entirely in Mix.** It lives where its kind
  says (`services/<role>d/`, `apps/<name>/`, `cli/<name>/`, `libs/<name>/`)
  and follows every naming rule. It has no Cargo package.
- **Its identity is `component.mx`** at the top of the component, a
  strict-data document (schema 1) with the fields a Cargo package carries
  in `[package.metadata.mixos]`, plus the name, version and principal
  program:

  ```text
  {
    schema: 1,
    name: "releasesd",          -- = directory = component
    component: "releasesd",
    kind: "service",            -- matches the kind directory
    layer: "desktop",           -- bus | mix | core | desktop
    contract: "public",         -- none | public
    version: "0.1.0",           -- the component's own MAJOR.MINOR.PATCH
    program: "scripts/releasesd.mix"
  }
  ```

- **A component is one or the other.** A directory with both `Cargo.toml`
  and `component.mx` is refused.
- **Layout inside the component:**
  - programs in `scripts/`, modules they `require` in `scripts/lib/`;
  - units in `units/`, calling `/opt/mixos/bin/mix` by absolute path;
  - tests in `tests/*_test.mix`.
- **Install location:** a Mix component's `scripts/` tree installs to
  `/opt/mixos/lib/<component>/`, read-only like the binaries in
  `/opt/mixos/bin`. `/opt/mixos/share` stays for data.
- **Shared Mix test helpers** live in `tests/lib/`. The first is
  `private_bus.mix`, which starts a throwaway, owned noded broker for Bus
  tests of serve citizens.
- **Gates.** `tools/component_index.mix` reads both identity sources,
  applies the same identity, layer and contract rules to each, and checks
  that `program` names a `.mix` file inside the component. Names are unique
  across both kinds. A Mix component has no Cargo dependency closure, so its
  declared layer is checked at review: it may use only the `mix` binary, the
  Bus and the verbs of components at or below its layer.
- **Versions and contracts.** A Mix component carries its own version, since
  there is no workspace package to inherit one from. With
  `contract: "public"`, its Bus verbs and document schemas are registered
  like any other (`docs/spec/bus/verbs.conf.mix`, `docs/spec/<topic>/`).

## Why

Mix is the system's own language. A service that is mostly glue (talking
HTTP, checking hashes, moving files, answering the Bus) is shorter, clearer
and easier for an agent to change in Mix than in Rust, and Mix already has
the primitives. Until now every component had to be a Cargo package, so such
a service had a choice of being rewritten in Rust to fit the layout, or
hidden in another component's `scripts/`, where it has no owner, no index
entry and no gate. Neither is honest.

`component.mx` keeps the one rule that matters (directory = component =
name, with a declared kind, layer and contract), so the component index stays
the agent's single map of the tree.

## Consequences

- The first Mix component is `services/releasesd` (upstream application
  releases).
- The generic line-JSON bridge for external applications (AGENTS.md §4.1)
  will land the same way.
- `tools/component_install.mix` installs Mix components: `scripts/` to
  `/opt/mixos/lib/<component>/` and `units/` to `/etc/systemd/system`. It
  writes an install record, keeps one previous tree for rollback and has a
  `--check` for drift. `--root` targets an image or container tree. Units of
  Rust components still wait for the unit collector.
- Rejected: a separate top-level directory for Mix programs. It would
  classify by language, not by kind of artefact.
