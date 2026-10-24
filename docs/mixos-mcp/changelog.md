---
title: mixos-mcp — changelog
description: Release notes of the mixos-mcp component, mirrored from cli/mixos-mcp/CHANGELOG.md.
generated: by docs/_build/gen-site.mix from cli/mixos-mcp/CHANGELOG.md; do not edit
---

# Changelog

> Mirrored from [`cli/mixos-mcp/CHANGELOG.md`](https://github.com/markc/mixos/blob/main/cli/mixos-mcp/CHANGELOG.md), where releases update it. The component's page is [mixos-mcp](README.md). This page describes the current `main` branch of MixOS.

## Unreleased

- Replace the unserved application action/control tools with `app_actions`
  and `app_execute`, using the application's registry Bus commands. Forward optional
  command parameters unchanged and preserve application error messages.
- Error-text pass-through without the old `application rc=` prefix also applies
  to `app_describe`, `desktop_windows` and `desktop_window_wait` through their
  shared native helper.
- The indexd Bus-mode call timeout is now 120 seconds by default, previously
  the Bus library's 60 seconds. SSH bridge requests carry the configured timeout.
- Retry bridge failures once before delivery for every command; after delivery,
  retry only idempotent commands. Feedback and timeouts report an unknown outcome.
