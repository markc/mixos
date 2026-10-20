---
title: Decision — one system binary
description: The mix shell and every headless MixOS daemon ship as one binary, built by cli/system and installed as mix. Each daemon is a symlink named after its component, and the binary picks what to run from the name it was started as. Components keep their own crates, tests, units and versions.
---

# Decision — one system binary

Status: accepted 2026-10-12, **on hold 2026-10-20**. Amends [the source layout](2026-10-04-source-layout.md)
(AGENTS.md §2.3, §3.4, §6 and §9).

> **On hold (2026-10-20).** The single multicall binary is not to be deployed on
> any host of the fleet. The fleet runs separate binaries. The personality
> plumbing stays in the tree, and the decision is to be revisited at a future date.

> **Correction (2026-10-19): the fleet build target is glibc, not musl.** The
> "Fleet builds are static" item below does not describe current practice. cbc
> builds against the workers' glibc, and the shipped `mix` and `webd` binaries
> link dynamically against glibc. The governing plan
> (`_plan/2026-10-02-mixos-transplant-plan.md`, §3.0 "Build target for mesh
> nodes") allows static or musl only where a node's glibc is too old. Every
> node in use is new enough, so no musl build is planned. Treat the static
> musl item and the matching sentence in "Why" as superseded until this
> decision is amended.

## The decision

- **One binary, `mix`, carries the shell and every headless daemon.** It is
  built by a new host component, `cli/system` (package `system`, kind `cli`).
  The host's only code is the dispatcher and the global allocator.
- **A daemon is a personality, chosen by the name the binary was started
  as.** `noded`, `maild`, `webd` and the rest are symlinks to `mix` in
  `/opt/mixos/bin`. Each still runs as its own process, under its own unit
  and journal identity, because the kernel names a process after the name it
  was executed by. There are no daemon subcommands under `mix`: the shell's
  first-argument namespace stays the shell's.
  - `mix --personalities` lists what the binary contains, with each
    component's version.
  - An unknown name exits 2 and prints that list.
  - The shell's self re-execution passes `mix` as its name, so a re-exec from
    a daemon personality comes back as the shell.
- **Components keep their identity.** Each daemon keeps its own crate, tests,
  units, metadata, version and verb registry. Its library exports
  `pub fn main() -> ExitCode`; its own `[[bin]]` stays for development
  builds. `cli/mix-shell` becomes a library with no binary.
- **What goes in:** the shell, `noded`, and every headless base and server
  daemon as it arrives, including the headless desktop-profile services.
  **What stays out:** anything with a GUI stack (compd, the applications),
  and any daemon whose dependencies dwarf the rest. The install-profile
  gate's closure check enforces the first.
- **One personality per process.** No setup happens at link or load time.
  The async runtime, TLS provider, signal masks, panic hook and logger are set
  up in the personality's own `main`, never in a shared library's
  initialiser. Each personality passes its own logging identity and records
  its own build provenance. Every personality uses one allocator, mimalloc.
  A test starts every personality and checks its identity.
- **Fleet builds are static.** The shipped binary targets
  `x86_64-unknown-linux-musl`, fully static, so one file runs on every
  supported distribution regardless of its C library.
- **Shell tests test the shipped binary.** The shell's process-level tests
  live in `cli/system/tests/` and run `mix` as built by the host.

## Why

Every daemon links the same core: the async runtime, serialisation, the Bus,
the Mix evaluator, logging. Linked separately, each carries its own copy, and
every release links that core once per daemon. Measured on the first
two-personality build (the shell and `noded`), the combined binary was 11%
smaller than the two separate ones and its release link took 40% less time.
Both gains grow with each personality. A static musl build cost nothing in
size, startup or evaluator speed, and the same file ran on both
distributions tested.

One file also means one artifact per architecture, one hash to verify and one
version on every node. Running daemons keep the old file's inode, so an
upgrade restarts only the units whose component changed.

Memory is not part of the case: different personalities run different code,
so they share little.

## Consequences

- AGENTS.md §2.3: `cli/system` builds the `mix` binary. `cli/mix-shell` is
  the shell's library and builds no binary.
- AGENTS.md §3.4: a daemon may be installed as a personality, a symlink named
  after its component; units name the symlink.
- AGENTS.md §6: `cli/system` is the one package allowed a non-dev dependency
  on `kind = "cli"` packages.
- AGENTS.md §9: the bootstrap is `cargo build --profile release-fast -p system`.
- Build scripts that built `-p mix-shell` to get `mix` build `-p system`.
- The installer creates each node's personality symlinks from its install
  profiles, keeps the previous binary for rollback, and restarts only changed
  units. The release archive's manifest lists each personality with its
  component version.
