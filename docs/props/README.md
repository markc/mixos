---
title: props
description: Property read types, revisions, namespace storage and native Bus routing for MixOS services.
---

# props

Shared property read types, revisions and optional namespace storage.
[noded](../noded/README.md), Mix and [editd](../editd/README.md) use the read
surface. Services can add durable domain records through the same library
and native Bus. [logging](../logging/README.md)'s opt-in bridge uses the
SQLite store and router for its `log` namespace; the substrate also supplies
the storage and routing API needed by the forthcoming mail service port.

| Feature | Adds |
| --- | --- |
| default (empty) | Pure read types, redaction, diff and in-memory revision writes |
| `bus` | Read dispatch and native publication envelopes |
| `store` | Namespace schemas, capabilities, hooks, lifecycle, audit HMAC, `PropertyStore`, `MemoryStore` and `Runtime` |
| `sqlite` | `store` plus `SqliteStore`, `SqliteTableMapping` and `JsonValuesMapping` |
| `router` | `bus` and `store` plus `PropsRouter`, `SubscribeGranter` and per-namespace event dispatch |

Choose `sqlite` and `router` together for durable service namespaces.
Services supply their own domain-table mapping, authorisation policy,
subscription granter and publisher. The library opens no second transport.
The [property contract](../docs/spec/props/README.md) defines the wire
surface, error vocabulary and persisted table ownership; the
[ownership decision](../docs/decisions/2026-10-18-property-store-in-props.md)
explains the boundary with desktop settings.

Read-surface transplant provenance: commit
`e0297242305f3a3c3de09f1ca01e8faa771768da`, MIT OR Apache-2.0.

The source is [`libs/props`](https://github.com/markc/mixos/tree/main/libs/props).
