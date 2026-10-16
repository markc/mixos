---
title: mix-shell
description: The Mix shell library, holding the interactive line editor, the command classifier, the native-session bootstrap and the Bus citizen runtime that the mix binary runs.
---

# mix-shell

The Mix shell: interactive line and editor facilities, the strict command
classifier, the native-session bootstrap and the ABP citizen runtime. All
application calls go through noded; the retired per-application socket fast
path is omitted. The client supervisor is the shared implementation in
[bus](../bus/README.md).

The language itself is the [mix](../mix/README.md) library, and its manual
is at [/mix/](../mix/README.md).

## The binary

This package is a library. The `mix` binary is built by
[system](../system/README.md), which runs `mix_shell::main` when started as
`mix` ([decision](../docs/decisions/2026-10-12-one-system-binary.md)):
`cargo build -p system`. The shell's process-level tests live in
`cli/system/tests` and run that binary (`cargo test -p system`); the unit
tests and library-only tests stay here.

## Session and manuals

The bootstrap retains the frozen `COSMIX_SESSION_FD` marker, sealed memfd
name and native-session transcripts. Ordinary shells initialise no grant
owner. Bundled manuals are available offline and use the same directory
rule as the session's other installed resources. Explicit remote manual
selection remains available.

## Provenance

Transplanted from `src/crates/cosmix-mix` at cosmix
`e0297242305f3a3c3de09f1ca01e8faa771768da`, with the complete regression
corpus. The native integration fixture compiles noded's production modules
and Term's production sealed-FD implementation, rather than simulating
session RPCs.

The source is
[`cli/mix-shell`](https://github.com/markc/mixos/tree/main/cli/mix-shell).
