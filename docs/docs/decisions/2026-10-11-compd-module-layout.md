---
title: Decision — compd's module layout on entry
description: compd's crates enter with every mod.rs renamed to the 2018 layout; their self-named module files stay as a recorded, shrinking exception.
---

# Decision — compd's module layout on entry

Status: accepted 2026-10-11. Amends [the source layout](2026-10-04-source-layout.md)
(AGENTS.md §4, Rust modules).

## Context

AGENTS.md §4 forbids new `mod.rs` and `foo/foo.rs` files and says code
transplanted from elsewhere is renamed on entry. The compositor's crates
arrive written in an older layout: about 570 `mod.rs` files and about 270
self-named module files (`foo/foo.rs`, each the file of a module `foo::foo`).

## The decision

- **Every `dir/mod.rs` becomes `dir.rs` on entry.** Rust resolves `dir.rs`
  with its children in `dir/`, so the rename changes no module path. An
  explicit `#[path]` resolves against the directory of the file that holds
  it, so each one in a renamed file is rewritten (for example
  `#[path = "encode/dialog_unavailable.rs"]` in `graphics`).
- **Self-named module files stay, as a standing exception.** Each is the
  canonical file of a module `foo::foo`; removing it renames the module at
  every caller, which is churn a transplant should not carry. No lint can
  gate them either way (`clippy::self_named_module_files` enforces the
  opposite, `mod.rs` style).
- **The exception shrinks and ends.** It covers only the compd crates, and
  each crate leaves it when it is next substantially edited: its self-named
  modules are dissolved into their parents in a change of their own. No new
  self-named module file is added to any crate.
- **No `mod.rs` can come back.** Every compd crate carries
  `[lints.clippy] mod_module_files = "deny"`, as the rest of the workspace
  does, and clippy runs as a gate.

## Consequences

- The transplant stays a near-zero semantic change, so its tests prove the
  same behaviour they proved before.
- Readers of compd will meet `foo::foo` paths for a while.
