# Changelog

## 0.1.1 — 2026-10-18

- Port the JMAP, SMTP, IMAP, CalDAV and CardDAV daemon and its authentication,
  rules and Bayesian crates onto the shared MixOS libraries and private mds.
- Register 43 maild Bus verbs and preserve property-store and storage contracts.
- Retain mail-parser 0.11.5's 64-level nesting safeguard and its direct tests.
- Use MixOS config paths, build and fixture variables, delegation envelopes,
  blob headers, protocol banners and DAV sync-token namespace.
- Add the system-binary personality, server packaging, maild@.service and the
  maild sysusers declaration. No separate metadata-store operator CLI is shipped.
- Preserve all source tests and document the six inherited ignores; add two
  repository registry consistency checks.
