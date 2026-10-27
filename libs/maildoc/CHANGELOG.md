<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
# Changelog

## Unreleased — 2026-10-27

- Add `MailDocument`, `parse`, `write` and typed `Refusal` errors for inert
  Markdown mail documents using the Bus framing and nine mail headers.
- Bound input to 8 MiB, refuse routing and unknown headers, and validate
  address lists, JSON strings and absolute attachment paths without file I/O.
- Preserve significant quoted header spaces; normalise address separators,
  reference whitespace and trailing body whitespace on read.
