---
title: editor-model — changelog
description: Release notes of the editor-model component, mirrored from libs/editor-model/CHANGELOG.md.
generated: by docs/_build/gen-site.mix from libs/editor-model/CHANGELOG.md; do not edit
---

> Mirrored from [`libs/editor-model/CHANGELOG.md`](https://github.com/markc/mixos/blob/main/libs/editor-model/CHANGELOG.md), where releases update it. The component's page is [editor-model](README.md). This page describes the current `main` branch of MixOS.

<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
# Changelog

## Unreleased — 2026-10-28

- Match editd's initial scratch dirty state in FakeEditd: pathless opens,
  including empty scratches, remain dirty until saved. Disk-backed opens
  start clean.
