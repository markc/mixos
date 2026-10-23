---
title: toolkit — changelog
description: Release notes of the toolkit component, mirrored from libs/toolkit/CHANGELOG.md.
generated: by docs/_build/gen-site.mix from libs/toolkit/CHANGELOG.md; do not edit
---

> Mirrored from [`libs/toolkit/CHANGELOG.md`](https://github.com/markc/mixos/blob/main/libs/toolkit/CHANGELOG.md), where releases update it. The component's page is [toolkit](README.md). This page describes the current `main` branch of MixOS.

<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
# Changelog

## Unreleased — 2026-10-23

- Replace the modal Macros panel with an icon-anchored menu, title-only
  creation and a non-modal shared editor with save/run shortcuts.
- Report completed macros through notifyd, retaining output per notification
  and opening the output strip if notifications are unavailable.
- Reuse menu navigation and drawing for anchored menus, including secondary
  release, Shift+Enter editing and title-initial navigation.
- Accept the shared editor's palette role interface without a dependency cycle.

## Unreleased — 2026-10-21

- Publish command contract 0.2.0 executed bodies (v2, bounded params and result).
- Map authorisation refusals to legacy `CommandError::Denied`.
- Report stored shortcut and `more` chords in the keymaps grammar, with
  Ctrl, Alt, Shift modifier order, through both command adapters.
