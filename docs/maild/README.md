---
title: maild
description: JMAP, SMTP, IMAP and DAV mail service
---

# maild

Maild serves JMAP, SMTP, IMAP, CalDAV and CardDAV. Its library builds the
same runtime for the daemon and integration tests. Private crates provide
mail authentication, rules and Bayesian classification; metadata storage uses
the shared `mds` library.

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
All path-valued settings resolve `mixos:<root>/<relative>` through `paths`,
including database, blob and MDS storage, spam and rule files/directories,
inbound filters, legacy TLS/DKIM files, managed key roots and each TLS/DKIM
identity's paths. `database` aliases `database_path`. Logical root escapes
and unknown roots fail config loading; ordinary relative and absolute paths
keep their meaning. Node-config fallback and in-process runtime construction
use the same resolver.
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

Maild ships as its own binary, as declared in `tools/release_packages.mx`.
`tools/package.mix` includes the binary, service/socket units and sysusers
declaration, and the vendored parser's licences in the server archive.

`maild@INSTANCE.service` requires `noded@INSTANCE.service` and reads the same
`<config>/session/INSTANCE.env` file, including its `MIXOS_NODE_CONFIG` broker
configuration. It runs as the `maild` account. The unit uses
`<config>/maild/config.conf.mix`, creates `StateDirectory=maild`, and sets
`MIXOS_VAR=/var/lib/maild`; configured TLS material must be readable by maild.
The unit documentation URL points to this manual.

`maild.service` is the plain per-host form, for a host that runs the plain
`noded.service` rather than per-session brokers. It reads
`<config>/maild/config.conf.mix`, creates `StateDirectory=mixos/maild mixos/jmap`, and takes its data
paths from the maild config. Data paths the config leaves unset default
under `<state>/jmap`, and DKIM, TLS and rule state under `<state>/maild`; with
`ProtectSystem=strict` only those two directories are writable. The sysusers declaration adds maild to the
`mixos-tls` group (gid 510), which it shares with webd for the TLS keys.

## Socket activation

Both service forms retain `CAP_NET_BIND_SERVICE` under the decision's interim
clause, so upgrading preserves config binds on privileged ports. Activation
is available, but its per-host installation is not automatic.

The shipped socket templates have **no active ListenStream** and cannot bind
until an operator or installer renders the host's configured addresses:

| Template | Descriptor name | Config source |
|---|---|---|
| `maild-smtp@.socket` | `smtp` | every `smtp_inbound` address |
| `maild-submissions@.socket` | `submissions` | every `smtp_smtps` address |
| `maild-imaps@.socket` | `imaps` | every `imap_imaps` address |
| `maild-jmap@.socket` | `jmap` | `listen` |

For `maild@INSTANCE.service`, render a drop-in for each selected protocol's
`maild-PROTOCOL@INSTANCE.socket`. Copy **all** addresses for that protocol
from the instance's config; an inherited protocol replaces all its config binds.
For example, `maild-smtp@INSTANCE.socket.d/listeners.conf` contains:

```ini
[Socket]
ListenStream=
ListenStream=192.0.2.10:25
ListenStream=[2001:db8::10]:25
```

`Service=maild@%i.service` selects the matching instance. For plain
`maild.service`, use the `default` socket instance and add
`Service=maild.service` to each socket drop-in. No installer enables these
examples or chooses loopback/public addresses. Enable the rendered socket
instances explicitly during the host's planned service transition, after
checking for conflicts with already-bound config listeners. Each uses
`FreeBind=yes`. TLS material is required for `submissions` and `imaps`.
JMAP, CalDAV and CardDAV share the `jmap` HTTP listener and its TLS config.
There is no separate STARTTLS submission listener on port 587.

Maild consumes fd 3 onwards only when `LISTEN_PID` matches its process;
`LISTEN_FDS` and `LISTEN_FDNAMES` must describe named listening TCP sockets.
Several fds may share a protocol name. Unknown names, mismatched counts and
non-listeners fail startup. The activation environment is cleared before the
runtime or helpers start. Inherited listeners replace configured binds for
that protocol; protocols without inherited sockets still use their configured
addresses. STARTTLS requirements match the inherited socket's actual local
address, just as they do a configured bind.

Only after **every privileged listener** has a working enabled socket, or has
been moved to an unprivileged config port, install this service drop-in at
`maild.service.d/socket-capabilities.conf` (or
`maild@INSTANCE.service.d/socket-capabilities.conf`):

```ini
[Service]
CapabilityBoundingSet=
AmbientCapabilities=
```

Keep the capability until that host transition is complete. Removing it while
any configured privileged listener still binds itself makes startup fail.

## Uploading and sending with JMAP

Upload an RFC 5322 message to the session's `uploadUrl`, then call
`Email/import` with `accountId` and an `emails` map. Each entry has `blobId`,
non-empty `mailboxIds`, optional `keywords` (default `{}`) and `receivedAt`
(a UTCDate, or null for the default). The default date comes from the most recent Received header,
or the import time. `ifInState` checks the Email state before any writes.
Responses contain per-entry `created` or `notCreated` results, with id,
canonical blob id, thread id and byte size for each created email.

Duplicate content within an account is rejected atomically with
`alreadyExists` and `existingId`, including concurrent imports and imports
through different upload aliases. It leaves existing metadata and memberships
untouched, even when a retry requests different mailboxes or keywords. Maild
does not add memberships as a side effect of this rejection: a failed import
must remain safe to retry, and the client should explicitly patch
`existingId` with `Email/set` to add mailboxes. This preserves the atomic
duplicate/quota rules and avoids silently overwriting existing keyword state.
Missing or invalid blob, mailbox or keyword references produce
`invalidProperties`; invalid MIME produces `invalidEmail`. Uploaded aliases
and account-owned CAS blob ids are accepted, with account ownership checked.
Parser unwinds are contained and returned as `invalidEmail` for that entry.
The legacy `Email/set create` extension reports invalid MIME as
`invalidProperties` on `blobId`, a standard SetError.
Positive account mail quotas are checked inside the creation transaction;
concurrent imports cannot exceed them. Upload staging does not consume mail quota.
Unexpected storage failures return `serverFail`; a failed creation transaction
leaves no email, memberships or state changes behind.
These are the [RFC 8621 §4.8](https://www.rfc-editor.org/rfc/rfc8621.html#section-4.8)
import semantics; the earlier placeholder's successful duplicate-create
response was incorrect.

Submit the returned email id with `EmailSubmission/set`. This works without
maild's legacy top-level `blobId` extension on `Email/set create`. The Email
creation id can also be referenced as `#draft` by submission in the same
request; mailbox creation ids resolve in import's `mailboxIds`. Implemented
`/set` methods also resolve Id fields, update/destroy targets and mailbox
parents, ordering dependent creates within one call. Unresolvable references
produce per-entry errors. Submission `onSuccessUpdateEmail` and
`onSuccessDestroyEmail` refer to successful submission ids, including `#id`
references, and return the implicit `Email/set` response with the same call id.
An optional
request `createdIds` map seeds these references and is returned with the new ids.
The Email
app still uses that extension: it should upload, import into Drafts with
`$draft`, then submit the created id (and handle import errors, including
`alreadyExists`). This branch does not change the app. Broader existing JMAP
projection gaps, including some address/header fields, remain separate work;
this is not a claim of full JMAP conformance. Thread ids are fetched in one
read per Email/get batch; a missing legacy thread row projects null. SQLite,
blob reads and MIME parsing run off the async executor.
