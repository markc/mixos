# Changelog

## Unreleased

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
