---
title: maild — changelog
description: Release notes of the maild component, mirrored from services/maild/CHANGELOG.md.
generated: by docs/_build/gen-site.mix from services/maild/CHANGELOG.md; do not edit
---

# Changelog

> Mirrored from [`services/maild/CHANGELOG.md`](https://github.com/markc/mixos/blob/main/services/maild/CHANGELOG.md), where releases update it. The component's page is [maild](README.md). This page describes the current `main` branch of MixOS.

## 0.1.1 — 2026-10-18

- Add the mail service skeleton and private metadata store, including SQLite
  migrations, blob storage, transactions, notifications and recovery tests.
- Keep storage operations in the library; no separate operator CLI is shipped.
