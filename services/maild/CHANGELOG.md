# Changelog

## Unreleased

- Keep SMTP alias fan-out accepted when another target's Sieve script rejects;
  send a DSN for that rejection instead of returning 550 after a committed copy.
  Log single-target Sieve SMTP refusals through the normal rejection logger.
- Deliver IPv4 and IPv6 address literals directly on port 25, including queued
  DSNs, with the existing outbound TLS and source-address policy.
- Index submission creation dates for retention sweeps. The first start with
  receipt retention enabled also expires pre-receipt EmailSubmissions older
  than the configured window (30 days by default).
- Run account Sieve on initial local submission and local retries, and create
  delivery receipts atomically for Sieve-generated SMTP replies.
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
- Migrate legacy queues to non-reusable IDs and bind receipts atomically.
  Retry failed local targets without repeating successful fan-out deliveries;
  show each local target in submission status. Expire terminal receipts and
  EmailSubmission records after configurable `delivery_retention_days` (30 by default).
- Derive BODY=8BITMIME from the body alone, using the first LF or CRLF header
  separator when identifying UTF-8 headers. Accept legacy raw eight-bit headers
  inbound and on submission; preserve transactions after rejected MAIL commands.
  Accept IPv4/IPv6 address literals syntactically. Skip dangling alias targets
  with an error log when healthy local targets remain, and check both IDN
  spellings when preventing account/alias collisions.
- Advertise SMTPUTF8 on inbound and submission, accept UTF-8 envelopes and
  headers, and negotiate SMTPUTF8 and BODY=8BITMIME on outbound delivery.
  Unsupported peers produce a permanent bounce; messages are not downgraded.
  Preserve valid MAIL AUTH trace parameters alongside SMTPUTF8.
- Generate RSA DKIM DNS keys as SubjectPublicKeyInfo, retain raw Ed25519 keys,
  and add `maild dkim check DOMAIN` to compare the published active key.
- Add `maild.submission.status {id}` with durable per-recipient queue outcomes
  and SMTP replies. Retries skip recipients already delivered or bounced.
- Resurrect deleted aliases, add `maild alias set`, local multi-target aliases
  and `@domain` catch-alls. Real mailboxes win over aliases and catch-alls;
  non-local targets are refused. Account inspection includes the numeric id.
  The read-only Bayesian unknown-account refusal already existed; native CLI
  regression coverage now checks its exact error text and absence of file creation.

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
