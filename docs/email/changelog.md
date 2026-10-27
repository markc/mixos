---
title: email — changelog
description: Release notes of the email component, mirrored from apps/email/CHANGELOG.md.
generated: by docs/_build/gen-site.mix from apps/email/CHANGELOG.md; do not edit
---

# Changelog

> Mirrored from [`apps/email/CHANGELOG.md`](https://github.com/markc/mixos/blob/main/apps/email/CHANGELOG.md), where releases update it. The component's page is [email](README.md). This page describes the current `main` branch of MixOS.

## Unreleased

- Pin received attachments below the reader body, with a remembered expanded
  tile view, Save All/Open All, per-file Open/Save and transfer status.
  Message › Attachments now toggles the bar's expansion.
- Show attached PNG, JPEG, GIF (first frame) and WebP images after the text
  by default. View › Show Attachments Inline is remembered across restarts.
  Previews use capped downloads, two background decode slots, explicit image
  limits and textures kept only for the open email; remote URLs stay unfetched.
- Report attachment bar choices and each preview's state/reason in
  `email.info` under `mail.reader`; both choices work through `email.execute`.
- Add an Attachments menu inside compose, with Attach files… (Ctrl+Shift+A),
  Remove all attachments and a one-click paperclip button. The shared chooser
  runs above compose and remembers the last attachment directory this session.
- Add `email.compose.attachment.add {paths}` and dropped-file attachment entry.
  They share atomic path validation, deduplication, draft revisions, metadata
  refresh and size warnings; sending refuses changes.
- Use citizen's shared app-launch helper when opening Prefs, preserving app
  refusals and avoiding repeated commands after an uncertain outcome.

- `email.execute` refuses `mail.send` when final validation prevents queueing,
  returning the compose error rather than a successful `executed` reply.
- Compose attachment metadata keeps the plain basename in `name` and the
  qualified display text in `label`. Failed sends clear their audit origin.
- Mixed local/drive input passes retain the drive caller and request id in
  send audits, with `mixed_input=true` recording the ambiguity. Other send
  audits use `mixed_input=false`.
- Narrow chip labels retain at least four em of width and truncate their text.
- Attachment chips wrap within the compose form's field width. Scoped chip
  frames no longer expand the row and dialog instead of starting a new row.
- `email.send` and `email.compose` accept an optional `attachments` list of
  absolute file paths. Symlinks are followed; relative paths, non-files,
  unreadable files and oversized messages have distinct refusal codes.
- Send reads files at send time and emits multipart MIME with desktop MIME
  types, base64 and RFC 2231 filenames. The encoded message is limited by
  JMAP `maxSizeUpload`, or 25 MiB when the server advertises no limit.
- Local drafts save and restore attachment paths (never file contents),
  including across restarts. Older drafts still load without attachments.
- Send resolves an earlier uncertain submission before checking local files or
  upload limits. Attachment reads and encoding run off the async runtime;
  command validation uses metadata only, and MIME bytes are rendered once.
- Standalone MIME builders impose no upload limit; Send applies the server's
  limit to the exact encoded bytes.
- Sends with attachments log the caller key and paths in the Email app.
  Person-initiated sends use `window`; `mail.send` through `email.execute`
  retains the Bus caller. Driven sends retain the actual replayed request id
  and caller through compose state and queued work for audit correlation.
- Compose shows attachment chips under Subject with names, sizes, file status,
  removal and a total. Missing, unreadable and invalid files or an oversized
  encoded message disable Send. Chips wrap, preserve text line height and
  disambiguate colliding filenames with their parent directories. Oversize
  warnings appear on the total line, with distinct account/default limits.
  Files are checked on open, removal, send failure and before Send;
  text edits update the estimate from cached sizes without polling files.
- `email.info` and the compose reply include the attachment metadata, total,
  warning and Send enablement. `email.compose.attachment.remove {path}` removes
  a path through the same state mutation as the chip's remove button.
- Attachment-only drafts are kept too. Restoring a vanished file preserves its
  path and shows it as missing; draft schema 1 remains compatible.
- Invalid logging configuration reports its error and lets the GUI start
  without a subscriber.
- MIME size estimates reserve the maximum 255-byte media type per part,
  without loading the MIME database or reading file contents. Send restores
  name/content sniffing on its worker; estimates bound actual MIME size with
  under 300 bytes of headroom per part.
