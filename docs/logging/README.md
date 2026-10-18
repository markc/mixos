---
title: logging
description: Shared tracing, sink selection, reload handles and process statistics for MixOS services and the Mix shell.
---

# logging

Shared tracing, sink selection, reload handles and process statistics for
MixOS services and the Mix shell. The default build has no Bus or HTTP
listener; `bus-handlers`, `prometheus` and `props` are explicit features. Log files
use the directory rule in [config](../config/README.md), including isolated
session roots.

Transplant provenance: commit
`e0297242305f3a3c3de09f1ca01e8faa771768da`, MIT OR Apache-2.0. Production callers are Mix and
[noded](../noded/README.md).

## Property-controlled logging

Enable `props` to register the `<svc>.log` singleton against the service's
[property store](../props/README.md). Call
`logging::props::register(&mut router, &store)` to obtain `LogNamespace`,
then await `logging::props::attach(&handle, namespace)` on the service's
Tokio runtime. Registration and read failures return `LogError::Props`.

Accepted `level` and `filter` writes update the live `EnvFilter`.
`level=none` installs `off` while preserving the subscriber for later
changes. Hooks reject malformed filters, unknown enum variants and deletes.
The watcher uses a namespace-private `Notify`, so it does not compete with
the property dispatcher. It lives until the Tokio runtime shuts down.

`format` remains stored introspection data and does not rebuild sinks.
`applied_at`, `applied_by` and `source` remain mutable metadata; the watcher
does not write them back. The current log policy grants every reachable
peer the namespace's capabilities, including write access that can silence
logging. Services needing narrower access must supply their policy.
See the [contract](../docs/spec/props/README.md#logging) for the frozen schema.

The source is [`libs/logging`](https://github.com/markc/mixos/tree/main/libs/logging).
