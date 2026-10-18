---
title: maild
description: JMAP, SMTP, IMAP and DAV mail service
---

# maild

Maild serves JMAP, SMTP, IMAP, CalDAV and CardDAV. Its library builds the
same runtime for the daemon and integration tests. Private crates provide
mail authentication, rules, Bayesian classification and metadata storage.

## Properties

Maild uses the shared `props` library with its `store`, `sqlite` and `router`
features. A `SqliteStore` named `maild` and a `PropsRouter` with service name
`maild` own the `account_overrides`, `accounts`, `aliases`, `domains`,
`engine_config`, `retention`, `tls_identities` and `log` namespaces. The
namespace modules register their schemas, SQLite mappings and hooks during
`build_runtime`; the logging library registers `log` through
`logging::props::register`.

The router handles `maild.props.*` requests. Durable changes publish on
`maild.props.records.changed` and `maild.props.audit`; replay and audit
semantics, capability strings, error tokens and the five `__props_*`
SQLite table names follow the shared props contract unchanged. Account,
alias and domain hooks reconcile the mail store after committed changes.

## Storage

`Mds` is the storage interface. `SqliteCasMds` stores per-set metadata in
SQLite, shares a content-addressable blob store, and emits notifications
after transactions commit. The library supports verification, garbage
collection, index rebuilding, statistics and set archive import/export.

The archive format retains its original v1 identifier for compatibility with
existing exports. The GC quiescence override is `MDS_GC_QUIESCENCE_SECS`;
it accepts integer seconds of at least five, with a default of 60 seconds.

Run `cargo test -p mds` on a build worker. The `mem-store` feature exposes
an in-memory implementation for consumers' tests. `_stress-helper` enables
the internal `mds-stress-writer` and its Unix SIGKILL recovery test; this
helper is never installed.
