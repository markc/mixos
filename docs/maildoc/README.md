---
title: maildoc
description: Inert Markdown mail documents using ABP framing, with a strict mail-header allowlist and no transport or file access.
---

<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
# maildoc

`maildoc` reads and writes the portable mail document defined by the
[mail document decision](../docs/decisions/2026-10-27-mail-document.md).
It uses [bus](../bus/README.md)'s wire parser with the client feature disabled.
Opening a document never sends mail. The library does no file access or
transport work; Email checks sender identities and real addresses when sending.

The source is [`libs/maildoc`](https://github.com/markc/mixos/tree/main/libs/maildoc).

## API and format

`parse(&str) -> Result<MailDocument, Refusal>` reads the document;
`write(&MailDocument) -> Result<String, Refusal>` serialises it;
`validate(&MailDocument) -> Result<(), Refusal>` checks canonical fields and
encoded size for Email and ced without file I/O. `MailDocument` derives
`Default`, `Clone`, `Debug`, `PartialEq` and `Eq`.

```text
---
mail-attachments: ["/home/user/report.pdf"]
mail-from: ann@example.org
mail-references: <first@example.org> <parent@example.org>
mail-subject: "[PATCH] build notes"
mail-to: Alice <alice@example.com>, bob@example.org
---
Hi Alice,

The **build** is ready.
```

Only these nine names are accepted:

| Header | Field | On disk |
| --- | --- | --- |
| `mail-to`, `mail-cc`, `mail-bcc` | `Vec<String>` | Comma-separated addresses, each trimmed; display text retained |
| `mail-from`, `mail-reply-to` | `Option<String>` | One address as written |
| `mail-subject` | `String` | Subject text |
| `mail-in-reply-to` | `Option<String>` | Parent message ID |
| `mail-references` | `Vec<String>` | Whitespace-separated message IDs, in order |
| `mail-attachments` | `Vec<PathBuf>` | JSON array of absolute UTF-8 paths; never opened |

Address validation here checks only that each entry is non-empty and contains
`@`, is trimmed and contains no comma. Commas always separate list entries, including commas inside display names;
full RFC address parsing belongs to Email.

An ordinary value starting with `"` must be a JSON string and is decoded.
Other ordinary values are taken as written after the wire parser trims them;
even an unquoted `[PATCH]` is accepted. The wire parser's advisory JSON errors
do not reject ordinary mail text. Attachments must always be a valid JSON
array of strings, never a JSON string containing an array. Decoded values,
including each attachment path, must contain no control characters (including
CR, LF and tab). Raw header values have the same restriction.

Every other header is refused by name, including unknown `mail-*` names and
all routing or broker names. Malformed and duplicate header lines are refused
before the lenient parser can skip or overwrite them. Empty values, including
`""` and bare `mail-subject:`, are refused; omit the header instead. Empty
lists are accepted and disappear when written. Both fences are required.
Any CR anywhere in the document, including bare CR and CRLF, has a separate
`Refusal::CarriageReturn` error with guidance to use LF.

Input larger than `MAX_DOCUMENT_BYTES` (8 MiB, the broker relay frame cap) is
refused with `Refusal::TooLarge`. The bound counts UTF-8 bytes for the complete
document, including framing; exactly 8 MiB is accepted. It applies locally too.

## Writing and round trips

The writer sorts headers through `bus::wire::BusMessage`, omits empty or absent
values, and uses LF framing. It JSON-quotes ordinary values beginning with
`[`, `{` or `"`, or carrying leading/trailing Unicode whitespace. Attachment
arrays remain JSON arrays. The body is appended exactly as supplied, with no
extra final newline. An empty header block includes a blank line between its
fences so the Bus parser can read a body following it; compact empty blocks
are accepted on read too.

Reading normalises these details:

- Unquoted values lose leading/trailing Unicode whitespace; quoted strings
  retain it. Address-list entries are trimmed after decoding.
- Address separators become comma plus space when written. Reference
  separators become one space. Their original separator bytes are not kept.
- The entire body's trailing Unicode whitespace is removed by
  `wire::parse_lenient`: spaces at the very end and trailing blank lines
  disappear. Leading whitespace and interior line endings/spaces survive.

For example, a body `hello  \n\n` reads as `hello`. Before writing that parsed
document, the caller gets the normalised body `hello`. The writer refuses a
body with trailing whitespace rather than silently changing its content.

`write` refuses non-canonical input. Header values contain no controls;
all address entries (including From/Reply-To) are trimmed, non-empty, contain
`@` and no comma; optional strings are non-empty; reference entries are
non-empty whitespace-free tokens; attachment paths are absolute UTF-8 without
controls; the body has no CR or trailing whitespace; and the encoded document
fits 8 MiB, including any expansion from JSON quoting. Non-UTF-8 paths return
`InvalidValue { header: "mail-attachments", reason: "path is not UTF-8" }`.
Parsing applies the same checks after normalisation. Every successful write
parses back to an equal document: `parse(&write(&d)?)? == d`.

Adding these names to `bus::wire::KNOWN_HEADERS` only silences advisory
`validate()` warnings. This crate's allowlist enforces the document boundary.

## Tests

Run `cargo test -p maildoc -p bus --locked` through the build cluster.
Unit tests cover refusals, quoting, normalisation, framing, sorted headers
and the relay bound; proptest checks canonical document round-trip equality
and that arbitrary documents either fail writing or round-trip unchanged.
