---
title: Mix regression tests (former mix-bench)
description: Current Mix interpreter, shell and process-level regression test locations; the former standalone mix-bench package is not part of this workspace.
---

# Mix regression tests

The former standalone `mix-bench` Cargo package is not in the current
MixOS workspace. This page keeps its existing documentation address so
older links resolve, but its old package name, sibling-directory lookup
and score command do not describe a tool that can be built here.

The current regression suites live with the components they exercise:

| Source | Coverage | Cargo package |
| --- | --- | --- |
| `libs/mix/src` and `libs/mix/tests` | Lexer, parser, evaluator, builtins, strict data and language regression scripts | `mix` |
| `cli/mix-shell/src` and `cli/mix-shell/tests` | Shell library and lint coverage | `mix-shell` |
| `cli/system/tests` | Shipped executable, native sessions, processes, manuals and shell integration | `system` |

In the designated Rust build environment, run the relevant suites with:

```text
cargo test --locked -p mix -p mix-shell -p system
```

These are correctness tests, not a replacement performance score. Check
the current manifests and test sources when selecting a workload.

See the [language library](../lib/README.md),
[shell library](../../mix-shell/README.md) and
[system executable](../../system/README.md).
