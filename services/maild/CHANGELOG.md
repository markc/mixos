# Changelog

## Unreleased

- Coalesce IMAP FETCH UID runs into indexed range scans, including one scan for
  full-folder requests with UID holes. Preserve IDLE sessions after transient
  resynchronisation failures and clear saved SEARCH results before error paths.
- Cover cross-session IMAP visibility of JMAP, SMTP and Sieve changes, and Sieve
  account-deletion cascades on both application and property-store connections.

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

- Complete the IMAP core with MIME part FETCH, non-extensible BODY and macros,
  BINARY decoding, ENABLE, ESEARCH/SEARCHRES, STATUS aggregates and APPENDLIMIT,
  LIST-EXTENDED/LIST-STATUS and UTF-8/modified UTF-7 mailbox names.
- Generalise literal framing and add atomic MULTIAPPEND and COPY. RENAME INBOX
  moves messages while preserving INBOX; subscriptions survive rename/delete.
  Correct CLOSE, EXAMINE, sequence-number notifications and shutdown BYE.
- Allocate new mailbox UIDVALIDITY values with a durable high-water mark so
  rapid delete/recreate cannot reuse a message identity. Convert legacy mailbox
  values to their existing wire seconds without invalidating client caches;
  repair only renamed identities that collide with live or deleted-name values.
- Avoid full-folder reads for unchanged selected sessions and indexed FETCH
  requests. IDLE flag notifications read one membership, with a catch-up diff
  only for an unknown UID.
- Report exact RENAME descendants with OLDNAME, accept explicit UTF-8/ASCII
  SEARCH charsets in every mode, clear failed SAVE state, and deselect a deleted
  selected mailbox. Reject non-canonical UTF-7 and C1 mailbox controls; correct
  subscription child attributes, delimiter probes and unpadded BINARY decoding.
- Bound SEARCH content work and match without whole-message lowercase copies.
  Keep CONDSTORE/QRESYNC gated on the separate extension implementation.
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
  the rest of the command can no longer be framed. The same framing now serves
  other command arguments and MULTIAPPEND. `BODY` and `TEXT` also search forwarded
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
