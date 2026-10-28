---
title: clipd — changelog
description: Release notes of the clipd component, mirrored from services/clipd/CHANGELOG.md.
generated: by docs/_build/gen-site.mix from services/clipd/CHANGELOG.md; do not edit
---

# Changelog

> Mirrored from [`services/clipd/CHANGELOG.md`](https://github.com/markc/mixos/blob/main/services/clipd/CHANGELOG.md), where releases update it. The component's page is [clipd](README.md). This page describes the current `main` branch of MixOS.

## 0.1.0 — 2026-10-28

- Replace the unmerged Rust prototype with a Mix `--serve` service.
- Add headless storage, both selections, text history, indexed picking,
  configured mesh follow and server push/pull, lazy password reads and expiry.
- Use external wl-clipboard tools for graphical sessions. Lazy remote Wayland
  ownership is unavailable and refused; see the manual before live acceptance.
- Reuse Bus-only Mix builtins and native MCP clipboard tools.
