---
title: maildoc — changelog
description: Release notes of the maildoc component, mirrored from libs/maildoc/CHANGELOG.md.
generated: by docs/_build/gen-site.mix from libs/maildoc/CHANGELOG.md; do not edit
---

> Mirrored from [`libs/maildoc/CHANGELOG.md`](https://github.com/markc/mixos/blob/main/libs/maildoc/CHANGELOG.md), where releases update it. The component's page is [maildoc](README.md). This page describes the current `main` branch of MixOS.

<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
# Changelog

## Unreleased — 2026-10-27

- Add `MailDocument`, `parse`, `write` and typed `Refusal` errors for inert
  Markdown mail documents using the Bus framing and nine mail headers.
- Bound input to 8 MiB, refuse routing and unknown headers, and validate
  address lists, JSON strings and absolute attachment paths without file I/O.
- Preserve significant quoted header spaces; normalise address separators,
  reference whitespace and trailing body whitespace on read.
