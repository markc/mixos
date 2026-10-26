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
| `store` | Namespace schemas, hooks, lifecycle, audit HMAC, `PropertyStore`, `MemoryStore` and `Runtime` |
| `sqlite` | `store` plus `SqliteStore`, `SqliteTableMapping` and `JsonValuesMapping` |
| `router` | `bus` and `store` plus `PropsRouter`, `SubscribeGranter` and per-namespace event dispatch |

Choose `sqlite` and `router` together for durable service namespaces.
Services supply their own domain-table mapping,
subscription granter and publisher. The library opens no second transport.
The [property contract](../docs/spec/props/README.md) defines the wire
surface, error vocabulary and persisted table ownership; the
[ownership decision](../docs/decisions/2026-10-18-property-store-in-props.md)
explains the boundary with desktop settings.

Every caller can read and mutate registered namespaces, including secret
fields and full schemas. Peer metadata supplies audit and live delivery
addresses; it does not grant access. A materialised view can declare
`NamespaceSpec::read_only`, which rejects every Bus mutation with
`validation_error` while allowing the owning runtime to refresh its data.

Schema validation, version checks, lifecycle hooks, audit digests, replay
windows and reserved topic ownership remain enforced. A live watch needs a
registered subscriber destination and a working subscription granter;
missing routing data returns `validation_error`, and a broker subscription
failure returns `grant_failed`. Public schema and broadcast event projections
still redact fields uniformly; callers can request full data explicitly.

Read-surface transplant provenance: commit
`e0297242305f3a3c3de09f1ca01e8faa771768da`, MIT OR Apache-2.0.

The source is [`libs/props`](https://github.com/markc/mixos/tree/main/libs/props).
