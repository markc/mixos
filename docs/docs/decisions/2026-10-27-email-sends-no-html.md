---
title: Decision — Email never creates HTML
description: MixOS Email sends text/plain only; a Markdown body goes out as plain text, never converted to HTML. This is a security choice, not a missing feature.
---

# Decision — Email never creates HTML

Status: accepted 2026-10-27.

## The decision

- **Email sends `text/plain` only.** The body is written in Markdown and
  goes out as that Markdown text, `text/plain; charset=utf-8`. Files are
  attached as their own parts under `multipart/mixed`.
- **No HTML part is ever generated.** Email does not render Markdown to
  HTML, and does not add a `multipart/alternative` HTML version, a
  stylesheet, inline images by `cid:`, tracking pixels or scripts.
- **A recipient without a Markdown-aware reader sees plain text.** That
  is intended. Markdown is readable as written.
- **This is a security choice, not a gap.** Do not add "HTML convenience",
  an HTML toggle, a rich-text composer, or a Markdown-to-HTML step to the
  send path. A change of this decision needs a new decision.

## Why

- HTML mail carries markup, CSS and sometimes script. Each is an attack
  surface for the reader at the other end, and a body that can be altered
  while it is relayed over SMTP between servers.
- Plain text has none of that. What the sender wrote is what is delivered,
  and any reader can show it safely.
- Markdown keeps the structure people want (headings, lists, links, code)
  without needing HTML to show it.

## What stays

- **Reading HTML mail.** Email still receives HTML mail from other
  clients. It shows the plain-text part first and prints any HTML as text.
  Nothing from a received email runs or loads.
- **Attachments.** A user may attach an `.html` file like any other file.
  It travels as an attachment, not as the message body.
