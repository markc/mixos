# Changelog

## Unreleased

- Fix Sieve attachment-independent header budgets, custom address fields,
  writable fileinto destinations, redirect-only loop suppression and private
  header removal on external redirects. Local submission stamps fresh spam
  metadata. Vacation replies have an independent default-on external policy,
  ManageSieve policy warnings, original-subject defaults and full recipient
  header recognition. Correct casemap ordering, discard/reject compatibility,
  oversized PUTSCRIPT quota responses and malformed redirect address refusal.
- Add per-account, versioned Sieve rules to SMTP and local submission delivery,
  with a bounded in-process interpreter, fileinto/create, flags, copy, body,
  variables, vacation, relational/index tests and reject/ereject. Script and
  policy errors keep the message. External redirects require an operator-enabled
  account domain; vacation suppression is durable and independently configured.
- Add an opt-in RFC 5804 ManageSieve listener using the shared TLS identities
  and account authentication, plus `maild.sieve.list/get/put/activate` Bus
  commands. SMTP's classifier and global Mix filter precede account Sieve;
  daemon-generated spam headers are available to scripts.
- Adopt named systemd TCP sockets for SMTP, implicit TLS submission, IMAPS
  and JMAP/DAV, retaining config binds without socket activation. Ship opt-in
  socket templates without active addresses. Retain the interim bind capability
  until each host explicitly enables sockets for all privileged listeners.
- Clear activation environment variables before starting runtime threads.
- Keep Email/import, blob creation and batched Email/get thread reads off the
  async executor; missing legacy thread rows project null. Accept null receivedAt
  as the import default. Map invalid MIME in Email/set create to
  invalidProperties on blobId.
- Resolve creation references at implemented /set Id fields and update/destroy
  targets, including dependent mailbox creates. Apply submission success actions
  through the implicit Email/set response.
- Resolve all configured paths, including TLS and DKIM identities, through
  the shared `mixos:` resolver; accept `database` as a `database_path` alias.
- Replace five JMAP placeholders with authenticated HTTP contracts. Correct
  `Email/import` (RFC 8621 §4.8) state preconditions, validation, CAS blob
  references, Received date defaults, thread ids and atomic duplicate rejection
  (`alreadyExists` with `existingId`), and transactional mail quota checks.
  Test upload/import/submission together. Unexpected storage errors return
  `serverFail` and roll back the entry, rather than blaming client arguments.
  Parser unwinds return `invalidEmail` without dropping the HTTP request.
- Resolve mail creation references across a JMAP request, including mailbox
  creation, import and submission, and preserve the optional `createdIds` map.
- Correct the manual and test guide to describe the packaged maild binary.

- IMAP `SEARCH` and `UID SEARCH` implement the header, date and text keys:
  `FROM`, `TO`, `CC`, `BCC`, `SUBJECT`, `HEADER`, `BODY`, `TEXT`, `BEFORE`,
  `ON`, `SINCE`, `SENTBEFORE`, `SENTON` and `SENTSINCE`. They were refused
  as deferred, so Thunderbird quick filters failed on every folder. Header
  keys match the raw and RFC 2047 decoded field; `BODY` covers every text
  and HTML part. A blob is read only for messages that flags, size and
  sequence keys leave undecided; an unreadable blob answers
  `NO [UNAVAILABLE]` rather than a partial result.
- `SEARCH` accepts string literals anywhere in the command, including several
  in one command (64 literals and 64 KiB at most). Past those limits a sync
  literal is refused; a non-sync one gets `BAD` and the connection closes, since
  the rest of the command can no longer be framed. Other commands keep the
  single trailing literal. `BODY` and `TEXT` also search forwarded
  `message/rfc822` parts. A literal may carry a multi-line needle; CRLF and LF
  match alike. A content scan stops when the client disconnects.

## 0.1.1 — 2026-10-18

- Port the JMAP, SMTP, IMAP, CalDAV and CardDAV daemon and its authentication,
  rules and Bayesian crates onto the shared MixOS libraries and private mds.
- Register 43 maild Bus commands and preserve property-store and storage contracts.
- Retain mail-parser 0.11.5's 64-level nesting safeguard and its direct tests.
- Use MixOS config paths, build and fixture variables, delegation envelopes,
  blob headers, protocol banners and DAV sync-token namespace.
- Add the system-binary personality, server packaging, maild@.service and the
  maild sysusers declaration. No separate metadata-store operator CLI is shipped.
- Preserve all source tests and document the six inherited ignores; add two
  repository registry consistency checks.
