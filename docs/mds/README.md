---
title: mds
description: Per-container-set SQLite metadata and a content-addressable blob store, the storage library that maild links and blobd will use. A library, not a daemon.
---

# mds

Per-container-set SQLite metadata and a content-addressable blob store.

`mds` is a library. It has no daemon of its own: maild links it, and blobd
will link it as the blob store. It provides the `Mds` trait, the production
implementation `SqliteCasMds` (one `data.sqlite` per set, one box-wide
`blobs.sqlite` index, and a flat CAS directory), and a `MemMds` for tests
behind the `mem-store` feature.

- Immutable content, multi-container mutable metadata, ordered access within
  a container, and a change log.
- Set and container lifecycle, blob put and get with BLAKE3 addressing, item
  add, copy, move and flags, change feeds, rebuild, verify, gc, export and
  import.
- Events are emitted after the durable commit, to an in-process sink. Publishing
  them on the Bus is the consumer's job.

The operator command-line binary is not ported yet.
