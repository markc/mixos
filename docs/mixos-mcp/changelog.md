---
title: mixos-mcp — changelog
description: Release notes of the mixos-mcp component, mirrored from cli/mixos-mcp/CHANGELOG.md.
generated: by docs/_build/gen-site.mix from cli/mixos-mcp/CHANGELOG.md; do not edit
---

# Changelog

> Mirrored from [`cli/mixos-mcp/CHANGELOG.md`](https://github.com/markc/mixos/blob/main/cli/mixos-mcp/CHANGELOG.md), where releases update it. The component's page is [mixos-mcp](README.md). This page describes the current `main` branch of MixOS.

## Unreleased

- The indexd Bus-mode call timeout is now 120 seconds by default, previously
  the Bus library's 60 seconds. SSH bridge requests carry the configured timeout.
- Retry bridge failures once before delivery for every verb; after delivery,
  retry only idempotent verbs. Feedback and timeouts report an unknown outcome.
