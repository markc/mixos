---
title: Decision — the mail document
description: A message saved by Email is a Markdown file framed as an ABP header block with mail-* headers only; it is never a live Bus request, and one address field never spans mail and the Bus.
---

# Decision — the mail document

Status: accepted 2026-10-27.

## The decision

- **A saved message is an ordinary Markdown file.** Its header block uses
  the ABP framing (`bus::wire`: a `---`-fenced block of `key: value` lines),
  followed by the Markdown body:

  ```text
  ---
  mail-attachments: ["/home/user/report.pdf"]
  mail-from: ann@example.org
  mail-subject: "[PATCH] build cluster: where things stand"
  mail-to: alice@example.com
  ---
  Hi Alice,

  The **cluster** is up.
  ```

- **Only `mail-*` headers.**
  - The accepted names are `mail-to`, `mail-cc`, `mail-bcc`, `mail-from`, `mail-reply-to`, `mail-subject`, `mail-in-reply-to`, `mail-references` and `mail-attachments`.
  - A file that carries any Bus routing header (`to`, `command`, `id`, `type`, `from`, `rc`, `reply-to`, `args`, `json`, and the broker's reserved names) is refused.
- **A mail document is not a request.** Opening, dropping or replaying the file never sends mail. Sending is always deliberate: Send in a composer, or `email.send {document: PATH}`.
- **The sender is checked.** `mail-from` and `mail-reply-to` must match one of the account's identities; otherwise Email refuses the document.
- **Values.**
  - Address lists are comma-separated.
  - `mail-attachments` is a JSON array of absolute paths, which Email reads at send time.
  - A value that starts with `[` or `{`, or has spaces that matter, is written as a JSON string.
  - Empty headers are omitted, and line ends are `\n`.
- **The body goes out as `text/plain`.** No HTML is created
  ([decision](2026-10-27-email-sends-no-html.md)).
- **One address field never spans mail and the Bus.** A mail recipient is a
  person reached by store and forward. A Bus address is a program on the
  mesh reached by request and reply. Raw Bus messages are composed in
  Workshop, never in Email.

## Why

- One parser and one format serve Email, ced, Mix and Workshop, and the file opens in any editor as Markdown with front matter.
- A file that is itself a live request would send mail whenever a tool replays ABP files. Keeping routing headers out makes every send an explicit act.
- Mixing the two address spaces in one field would make the safety of a send depend on the shape of a string.

## What stays

- Drafts are Email's private records, because they carry the account, identity and send attempt. The mail document is the Open… / Save As… format.
- `email.send` and `email.compose` keep their JSON arguments; `document` is an additional path argument.
