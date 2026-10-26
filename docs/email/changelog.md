---
title: email — changelog
description: Release notes of the email component, mirrored from apps/email/CHANGELOG.md.
generated: by docs/_build/gen-site.mix from apps/email/CHANGELOG.md; do not edit
---

# Changelog

> Mirrored from [`apps/email/CHANGELOG.md`](https://github.com/markc/mixos/blob/main/apps/email/CHANGELOG.md), where releases update it. The component's page is [email](README.md). This page describes the current `main` branch of MixOS.

## Unreleased

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
  Attachment controls will follow separately.
