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
- Compose keeps attachment paths in memory. Attachment controls and draft
  persistence will follow separately.
