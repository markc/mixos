---
title: system
description: The system package builds the mix shell executable; optional noded, monitord, maild, webd and dnsd personalities are off by default, and each daemon ships as its own binary.
---

# system

The system binary, `mix`: the Mix shell. It can also carry headless daemon
personalities in one file
([decision](../docs/decisions/2026-10-12-one-system-binary.md), on hold).

The default build is the shell alone (Mark, 2026-11-02): the `noded`,
`monitord`, `maild`, `webd` and `dnsd` features are off by default, and
every daemon, like `settingsd`, `hostd`, `portald` and `compd`, builds and
ships as its own executable (`cargo build -p noded` gives
`target/release/noded`). Units run those binaries, never a symlink to
`mix`. The one-system-binary
decision describes the architecture; the features in
[`cli/system/Cargo.toml`](https://github.com/markc/mixos/blob/main/cli/system/Cargo.toml)
and dispatcher are the current personality inventory.

The binary picks a personality from the basename of argv[0]. Started as
`mix` it is the shell ([mix-shell](../mix-shell/README.md),
`mix_shell::main`); started through a symlink named `noded` it is the node
daemon ([noded](../noded/README.md), `noded::main`), when that feature is
built in. Each daemon still runs
as its own process, under its own unit and journal identity. This package
holds only the dispatcher and the global allocator (mimalloc); each
component keeps its own crate, tests, version and units.

```text
cargo build --locked --profile release-fast -p system --bin mix
                                              # target/release-fast/mix, the shell alone
cargo build --locked --profile release-fast -p system --bin mix --features noded
mix --personalities                            # each personality and its component version
ln -s mix noded                                # noded, only if built with --features noded
```

An unknown name exits 2 and prints the personalities. A login shell's
`-mix` is the shell.

The Mix language is documented in the [Mix manual](../mix/README.md).

## Tests

`cargo test -p system` runs the shell's process-level tests, which moved
here from `cli/mix-shell` so they test the shipped binary. The identity test
(`tests/personalities.rs`) builds only with all five personality features
(`--features noded,monitord,maild,webd,dnsd`). Their scripts moved with them
(`tests/scripts`); the manual they check is the site source
([`docs/mix`](../mix/README.md)), and the source guards read
`cli/mix-shell/src`.

The source is [`cli/system`](https://github.com/markc/mixos/tree/main/cli/system).
