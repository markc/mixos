---
title: maild
description: JMAP, SMTP, IMAP and DAV mail service
---

# maild

Maild serves JMAP, SMTP, IMAP, CalDAV and CardDAV. Its library builds the
same runtime for the daemon and integration tests. Private crates provide
mail authentication, rules, Bayesian classification and metadata storage.

Accounts can run [Sieve delivery scripts](sieve.md), managed through the Bus or
the optional TLS ManageSieve listener on port 4190. Off-site redirects and
automatic Sieve replies are disabled until the operator allows the account domain.

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
semantics, error tokens and the five `__props_*`
SQLite table names follow the shared props contract unchanged. Account,
alias and domain hooks reconcile the mail store after committed changes.

Every Bus caller can read and write properties, including secret fields.
`tls_identities` is a materialised resolver view: its Bus mutations return
`validation_error` for every caller; internal reconciliation refreshes it.
Vtoken management, retention sweeps and Bayesian rebuilds accept every caller
without operator lists or delegation envelopes. Retention still validates
its armed account set, deletion windows, dry-run flag and sweep limits.

Submission accepts mesh requests with no `from` sender. Audit records use
the broker peer when supplied, otherwise the stable actor `bus`; the typed
request selects the sending account. Sender validation, size limits and
operation replay protection still apply. Config loading ignores unknown
keys, so existing files carrying removed operator lists continue to load.

## Shared libraries

Maild uses `bus::native_client::NodedClient` and the Bus connection helpers.
Its advertised commands are registered in the repository Bus registry and checked
by `tests/registry.rs`. Incoming commands retain their connection generation;
the typed port helper preserves the remote return code and parsed reply.

Configuration uses `config::Dir::{Etc, Var}` and `config::path`. The system
file is `<Etc>/maild/config.conf.mix`; strict-data loaders return typed errors.
Node-wide mail settings use the typed `config::node::NodeConfig` fields, with
the shared `config::tls` SNI resolver. Logging attaches the namespace returned
by `logging::props::register` through `logging::props::attach`.

Build provenance comes from `buildinfo` and its `MIXOS_*` build variables.
Mail filters use the shared `mix` interpreter with its `json` and `serde`
features. DAV uses `davproto` and the `mixos:rawICalendar` and `mixos:rawVCard`
keys; retired raw keys remain errors.

## Metadata store

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

## Running and packaging

The system binary carries maild as a personality: install `maild` as a symlink
to `mix`. The standalone development binary runs the same library entry point.
`tools/package.mix` includes that symlink, the template service and sysusers
declaration, and the vendored parser's licences in the server archive.

`maild@INSTANCE.service` requires `noded@INSTANCE.service` and reads the same
`<config>/session/INSTANCE.env` file, including its `MIXOS_NODE_CONFIG` broker
configuration. It runs as the `maild` account. The unit uses
`<config>/maild/config.conf.mix`, creates `StateDirectory=maild`, and sets
`MIXOS_VAR=/var/lib/maild`; configured TLS material must be readable by maild.
The unit documentation URL points to this manual.

`maild.service` is the plain per-host form, for a host that runs the plain
`noded.service` rather than per-session brokers. It reads the node config
from `<config>`, creates `StateDirectory=mixos/maild mixos/jmap`, and takes its data
paths from the maild config. Data paths the config leaves unset default
under `<state>/jmap`, and DKIM, TLS and rule state under `<state>/maild`; with
`ProtectSystem=strict` only those two directories are writable. The sysusers declaration adds maild to the
`mixos-tls` group (gid 510), which it shares with webd for the TLS keys.
