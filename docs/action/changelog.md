---
title: action — changelog
description: Release notes of the action component, mirrored from libs/action/CHANGELOG.md.
generated: by docs/_build/gen-site.mix from libs/action/CHANGELOG.md; do not edit
---

> Mirrored from [`libs/action/CHANGELOG.md`](https://github.com/markc/mixos/blob/main/libs/action/CHANGELOG.md), where releases update it. The component's page is [action](README.md). This page describes the current `main` branch of MixOS.

<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
# Changelog

## 0.3.0 — 2026-10-21

- Add bounded transactional `History<D>` snapshots, explicit coalescing,
  undo/redo labels and saved checkpoints for application engines.
- Count retained redo snapshots as well as undo in the history byte budget.
- Route Screenshot document edits through parameterised commands. The
  `screenshot.execute` body rejects unknown fields (`deny_unknown_fields`).
- Rename Screenshot's nullary Delete selection command from `edit.delete`
  to `edit.delete_selected`; keymap overrides naming the old command must
  move to `edit.delete_selected`. `edit.delete {id}` now deletes one object.

## 0.2.0 — 2026-10-21

- Emit only v2 executed bodies, with accepted params and returned result;
  independently replace compact values above 4096 bytes with bytes and SHA-256.
- Add the strict, schema-bearing `ExecuteRequest` body for `<app>.execute`.
- Return `Denied` on authorisation refusal before checking enablement.
- Canonicalise modifiers as Ctrl, Alt, Shift and report stored chord spelling
  through toolkit's `<app>.actions` adapter.

## 0.1.0 — 2026-10-20

- Extract the registry, Fluent strings and v1 executed journal from toolkit.
- Add parameter schemas, results, typed refusals, authorisation and classes.
- Preserve toolkit's nullary tables through its egui adapter.
