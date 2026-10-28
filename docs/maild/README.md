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

## SMTP internationalisation

Inbound SMTP and authenticated submission advertise `SMTPUTF8` and
`8BITMIME`. Send `MAIL FROM:<address> SMTPUTF8` for UTF-8 envelope addresses
or UTF-8 headers, adding `BODY=8BITMIME` for an eight-bit body. UTF-8 local
parts and domains are stored without transliteration or Unicode normalisation;
address lookup folds ASCII case as for existing mailboxes. Invalid UTF-8
commands and UTF-8 addresses/headers without the parameter are refused.
Configure hosted internationalised domains in `maild.domains` using their DNS
A-label (for example `xn--fsqu00a.example.com`); the RCPT gate converts the
envelope's U-label for that authoritative lookup.
Mailbox lookup accepts either the U-label or A-label spelling of its domain.

Outbound delivery detects non-ASCII envelope addresses and headers, checks the
peer's EHLO capabilities (again after STARTTLS), and requests `SMTPUTF8` when
needed, including UTF-8 MIME part headers. SMTPUTF8 and eight-bit content
require `8BITMIME` and request `BODY=8BITMIME`.
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
an optimistic version check. Concurrent changes require a retry. Both commands
take one to 32 local account addresses, with no chains or off-site forwarding.

Delivery checks for a real mailbox first, then an enabled exact alias, then
the enabled domain catch-all. All targets must still be real local accounts;
removing one causes a retriable delivery failure rather than external
forwarding. Multiple local targets receive a copy each. An exact alias grants
its targets permission to send as that alias; a catch-all grants no sender
identity. Reserved virtual-token recipients retain their existing routing.
Property records retain `target` as the first target for compatibility
and add `targets` for the full list; an absent or empty list uses `target`.

`maild account show alice@example.com` includes `account_id`, the numeric id
accepted by Bayesian commands. The read-only `maild.accounts.lookup {email}`
query supplies this identity without opening the account's mail store.
`maild bayesian stats ID` refuses unknown ids
and reads an existing corpus without creating a directory or `bayes.db`;
an existing untrained account reports cold-start counts.

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
must remain safe to retry. The current Email/set update path only supports
single-mailbox moves; it cannot add memberships to a multi-mailbox email.
Safe merging needs an atomic duplicate/membership operation with defined
per-membership keyword handling, preserving IMAP-only flags and the existing
quota rules. Until that operation exists, clients can use `existingId` for
supported updates or moves, but membership union is unavailable.
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
produce per-entry errors. Wrongly typed `create`, `update`, `destroy` or
`ifInState` arguments instead fail the whole method with `invalidArguments`
before any stage runs. A non-null `ifInState` must match the initial state;
otherwise the whole call returns `stateMismatch` without running any mutation
stage. Create, update and destroy stages run in that order. A `/set` is not one
transaction across those stages; concurrent requests may change data between
them, and a later per-entry failure does not roll back earlier successes.

Submission `onSuccessUpdateEmail` and `onSuccessDestroyEmail` name submissions,
including `#id` creation references. Under
[RFC 8621 §7.5](https://www.rfc-editor.org/rfc/rfc8621.html#section-7.5),
an action applies only when the corresponding submission operation succeeds
in that invocation. An existing submission id without a successful operation,
a reference from an earlier call, a failed creation or an unknown reference is
ignored. Such entries do not produce errors in the implicit `Email/set`.
Maild currently implements submission creation; submission updates and
destruction remain unsupported and cannot trigger these actions. Successful
actions return the implicit `Email/set` response with the same call id;
any email patch errors name the actual Email id, never the submission reference.
Explicit success actions suppress the legacy automatic move to Sent, so the
client's mailbox patch applies to the original draft memberships.
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
