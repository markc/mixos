---
title: logging
description: Shared tracing, sink selection, reload handles and process statistics for MixOS services and the Mix shell.
---

# logging

Shared tracing, sink selection, reload handles and process statistics for
MixOS services and the Mix shell. The default build has no Bus or HTTP
listener; `bus-handlers` and `prometheus` are explicit features. Log files
use the directory rule in [config](../config/README.md), including isolated
session roots.

Transplanted from `cosmix-lib-log` at cosmix
`e0297242305f3a3c3de09f1ca01e8faa771768da`. Production callers are Mix and
[noded](../noded/README.md).

The source is [`libs/logging`](https://github.com/markc/mixos/tree/main/libs/logging).
