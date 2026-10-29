---
title: workshop — changelog
description: Release notes of the workshop component, mirrored from apps/workshop/CHANGELOG.md.
generated: by docs/_build/gen-site.mix from apps/workshop/CHANGELOG.md; do not edit
---

> Mirrored from [`apps/workshop/CHANGELOG.md`](https://github.com/markc/mixos/blob/main/apps/workshop/CHANGELOG.md), where releases update it. The component's page is [workshop](README.md). This page describes the current `main` branch of MixOS.

<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
# Changelog

## Unreleased — 2026-10-28

- Preserve the final shutdown result when an asynchronous completion arrives
  after the command inbox has been closed and drained.
- Follow session font size and weight with a custom design package, keeping
  the theme file design beneath profile framing, menus and title band.
- Use the shared 360×240 logical window minimum. Stack the services pane in narrow tiles, size docks from remaining space, and scroll oversized dock content.
