---
title: system
description: The system package builds the mix shell executable with optional noded, monitord, maild, webd and dnsd personalities selected by the name it is started under.
---

# system

The system binary, `mix`: the Mix shell and its enabled headless daemon
personalities in one file
([decision](../docs/decisions/2026-10-12-one-system-binary.md)).

The default Cargo features include `noded`, `monitord`, `maild`, `webd` and
`dnsd`; each is optional. Services such as `settingsd`, `hostd`, `portald`
and `compd` still build their own executables. The one-system-binary
decision describes the architecture; the features in
[`cli/system/Cargo.toml`](https://github.com/markc/mixos/blob/main/cli/system/Cargo.toml)
and dispatcher are the current personality inventory.

The binary picks a personality from the basename of argv[0]. Started as
`mix` it is the shell ([mix-shell](../mix-shell/README.md),
`mix_shell::main`); started through a symlink named `noded` it is the node
daemon ([noded](../noded/README.md), `noded::main`). Each daemon still runs
as its own process, under its own unit and journal identity. This package
holds only the dispatcher and the global allocator (mimalloc); each
component keeps its own crate, tests, version and units.

```text
cargo build --locked --profile release-fast -p system --bin mix
                                              # target/release-fast/mix
mix --personalities                            # each personality and its component version
ln -s mix noded                                # noded, as its own process
```

An unknown name exits 2 and prints the personalities. A login shell's
`-mix` is the shell.

The Mix language is documented in the [Mix manual](../mix/README.md).

## Tests

`cargo test -p system` runs the identity test (`tests/personalities.rs`) and
the shell's process-level tests, which moved here from `cli/mix-shell` so
they test the shipped binary. Their scripts moved with them
(`tests/scripts`); the manual they check is the site source
([`docs/mix`](../mix/README.md)), and the source guards read
`cli/mix-shell/src`.

The source is [`cli/system`](https://github.com/markc/mixos/tree/main/cli/system).
