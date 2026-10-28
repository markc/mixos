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

## SMTP internationalisation

Inbound SMTP and authenticated submission advertise `SMTPUTF8` and
`8BITMIME`. Send `MAIL FROM:<address> SMTPUTF8` for UTF-8 envelope addresses
or UTF-8 headers, adding `BODY=8BITMIME` for an eight-bit body. UTF-8 local
parts and domains are stored without transliteration or Unicode normalisation;
address lookup folds ASCII case as for existing mailboxes. Invalid UTF-8
commands and UTF-8 addresses/headers without the parameter are refused.

Outbound delivery detects non-ASCII envelope addresses and headers, checks the
peer's EHLO capabilities (again after STARTTLS), and requests `SMTPUTF8` when
needed. Eight-bit content requires `8BITMIME` and requests `BODY=8BITMIME`.
ASCII mail does not request either extension. If a required extension is
missing, maild records a permanent `5.6.7` (SMTPUTF8) or `5.6.3` (8BITMIME)
failure and bounces to the local sender. It does not rewrite addresses or
content. This is the non-delivery policy allowed by
[RFC 6531 §3.2](https://www.rfc-editor.org/rfc/rfc6531#section-3.2).

## DKIM DNS checks

`maild dkim generate --domain example.com --selector s2026a` prints a TXT
value whose RSA `p=` contains DER SubjectPublicKeyInfo, as required by
[RFC 6376 §3.6.1](https://www.rfc-editor.org/rfc/rfc6376#section-3.6.1).
Ed25519 `p=` remains the raw 32-byte key from
[RFC 8463](https://www.rfc-editor.org/rfc/rfc8463#section-4).
Publish that value at `s2026a._domainkey.example.com`. DNS TXT chunks belong
to one record and must concatenate without extra whitespace inside the key.

Run `maild dkim check example.com` after publication. The read-only
`maild.dkim.check {domain}` query reads the active selector, fetches its TXT
record and compares its key and algorithm with the installed private key.
Success returns `matches:true`; a missing, duplicate, revoked or mismatching
record fails. Legacy RSA PKCS#1 DNS values must be republished as SPKI; private
keys remain valid and need no rotation. The query also supports configured
`[[dkim.domain]]` active signers.

## Submission status

`maild.submit` returns an acceptance receipt with `submission_id`.
`delivery_confirmed:false` in this replayable receipt describes acceptance;
it does not change when delivery finishes. Query current results with:

```text
maild.submission.status {"id":"00000000-0000-4000-8000-000000000001"}
```

The response has `id`, `delivery_confirmed` and `recipients`. Each recipient
has `recipient`, `status` (`queued`, `delivered`, `deferred`, `bounced`),
`last_smtp_reply` (null before an attempt) and `updated_at` (UTC). Local delivery
records the local completion reply. Remote delivery records the receiving
server's end-of-DATA reply or its rejection/error. Delivered means the next
server accepted responsibility, not that a person read the message.

Queue results survive payload removal and daemon restarts. Deferred recipients
retry with the queue's backoff; delivered and bounced recipients are skipped.
Each remote recipient has its own SMTP transaction, trading connection reuse
for independent results and retries. Unknown ids, malformed requests and
legacy submissions without recorded outcomes return an error, never an invented
delivery result. Terminal receipts are retained with submission records.

## Local aliases

```text
maild alias add sales@example.com alice@example.com bob@example.com
maild alias set sales@example.com bob@example.com
maild alias add @example.com alice@example.com bob@example.com
maild alias remove sales@example.com
```

`add` creates an absent alias, including resurrection of a tombstone at its
retained version; it refuses an existing alias. `set` creates or repoints with
an optimistic version check. Concurrent changes require a retry. Both verbs
take one to 32 local account addresses, with no chains or off-site forwarding.

Delivery checks for a real mailbox first, then an enabled exact alias, then
the enabled domain catch-all. All targets must still be real local accounts;
removing one causes a retriable delivery failure rather than external
forwarding. Multiple local targets receive a copy each. An exact alias grants
its targets permission to send as that alias; a catch-all grants no sender
identity. Property records retain `target` as the first target for compatibility
and add `targets` for the full list; an absent or empty list uses `target`.

`maild account show alice@example.com` includes `account_id`, the numeric id
accepted by Bayesian commands. `maild bayesian stats ID` refuses unknown ids
and reads an existing corpus without creating a directory or `bayes.db`;
an existing untrained account reports cold-start counts.

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
