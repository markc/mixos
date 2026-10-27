<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
# Changelog

## Unreleased — 2026-10-27

- Make `write` fallible and expose `validate`: refuse header injection,
  control characters, non-canonical fields, non-UTF-8 paths and oversized
  encoded output without panicking.
- Refuse bare CR as well as CRLF, and report colon-free header lines as
  malformed rather than forbidden names.
- Add `MailDocument`, `parse`, `write` and typed `Refusal` errors for inert
  Markdown mail documents using the Bus framing and nine mail headers.
- Bound input to 8 MiB, refuse routing and unknown headers, and validate
  address lists, JSON strings and absolute attachment paths without file I/O.
- Preserve significant quoted header spaces; normalise address separators,
  reference whitespace and trailing body whitespace on read.
