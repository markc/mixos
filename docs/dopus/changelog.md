---
title: dopus — changelog
description: Release notes of the dopus component, mirrored from apps/dopus/CHANGELOG.md.
generated: by docs/_build/gen-site.mix from apps/dopus/CHANGELOG.md; do not edit
---

> Mirrored from [`apps/dopus/CHANGELOG.md`](https://github.com/markc/mixos/blob/main/apps/dopus/CHANGELOG.md), where releases update it. The component's page is [dopus](README.md). This page describes the current `main` branch of MixOS.

<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
# Changelog

## Unreleased — 2026-10-28

- Follow session font size and weight with a custom design package, keeping
  the theme file design beneath profile framing, menus and title band.

## Unreleased — 2026-10-27

- Fix chooser name-field Enter and initial selection, Escape precedence,
  rejected acceptance recovery and the five-second native mapping deadline.
  Validate drive viewports at arrival and move narrow placement capture off
  the Bus command path. Share the MIME catalogue across chooser windows.
- Display caller text safely, load saved Places read-only, count filtered
  rows and use a primary Accept button with plural-aware replace messages.
  Split slash-containing save names into a folder and retained name.
- Add native file chooser windows in `--choose-service`, with shared DOpus
  panes, filters, caller choices, replacement confirmation and request-scoped
  toolkit drive commands. Cover both themes and a private nested compositor.
- Add Mail selection to File, the row context menu and Ctrl+M, available for
  selections of regular files (symlinks followed). Listing workers supply
  regular-file metadata, keeping availability checks off the filesystem.
- Pass attachment paths to Email over the existing Bus task, launching it
  through citizen when absent. BUSY falls back to adding to the open compose.
  Status and Bus replies report the outcome; headless calls share this path.
