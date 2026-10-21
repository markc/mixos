---
title: Source and build map
description: The current Mix interpreter, shell, executable, application and documentation source paths, how builds relate to installed versions, and where staged decisions need implementation checks.
---

## Read first

This manual describes source on `main`; installed binaries can be older or
built from another revision. Check the installed program's version and its
live `app.describe` or `HELP` response before relying on an API. An accepted
decision records the intended contract and can describe work still being
implemented; it is not proof that every planned feature is available.

- [Repository rules and layout](https://github.com/markc/mixos/blob/main/AGENTS.md)
- [Mix gotchas](https://mixos.dev/mix/gotchas.md): read before writing Mix.
- [Component index](https://mixos.dev/docs/components.md): source paths, kinds and install profiles.
- [Driving apps over the Bus](https://mixos.dev/docs/drive.md): discovery, commands and captures.

## Source and build map

One public repository, [markc/mixos](https://github.com/markc/mixos), holds one
Cargo workspace and the site source. Paths below are relative to its root.

- `libs/mix/`: the `mix` Cargo package and Rust library, with the lexer,
  parser, evaluator, builtins and language regression tests.
- `cli/mix-shell/`: the `mix-shell` library, owning the interactive shell,
  script runner, command dispatch and supervised Bus citizen runtime.
- `cli/system/`: the `system` package that builds the `mix` executable.
  Its default daemon personalities are `noded`, `monitord`, `maild`, `webd`
  and `dnsd`. Other services, including `settingsd`, `hostd`, `portald` and
  `compd`, have their own packages and executables.
- `apps/`: GUI applications; `services/`: daemons; `libs/`: shared libraries.
  GUI apps use egui/eframe and wgpu, with common widgets in `libs/toolkit/`.
  Each app links its libraries into its own executable: shared widget code
  changes require rebuilding and restarting each affected app.
- `docs/mix/`: the public Mix manual; `docs/_build/gen-site.mix`: the native
  Mix site generator, including this index and `llms-full.txt`.

In the designated Rust build environment,
`cargo build --locked --profile release-fast -p system --bin mix` produces
`target/release-fast/mix`. `cargo build -p mix` builds the language library,
not the shell executable. Installed binaries belong under `/opt/mixos/bin`.
Mix programs run directly with `/opt/mixos/bin/mix script.mix`; editing a
program does not require compiling Rust. Use `mix builtins NAME` for builtin
contracts, `run_argv` or `run_argv_must` for processes and `ssh_mix` for remote
Mix programs.

The `.mix` program / `.mx` document split is an accepted migration target.
Existing readers still use legacy names such as `*.conf.mix` and `scene.mix`;
check the owning reader before renaming data or assuming a loader's behaviour.
