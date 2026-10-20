<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
# Changelog

## 0.2.0 — 2026-10-21

- Emit only v2 executed bodies, with accepted params and returned result;
  independently replace compact values above 4096 bytes with bytes and SHA-256.
- Add the strict, schema-bearing `ExecuteRequest` body for `<app>.execute`.
- Return `Denied` on authorisation refusal before checking enablement.
- Canonicalise modifiers as Ctrl, Alt, Shift and report stored chord spelling
  through toolkit's `<app>.commands` adapter.

## 0.1.0 — 2026-10-20

- Extract the registry, Fluent strings and v1 executed journal from toolkit.
- Add parameter schemas, results, typed refusals, authorisation and classes.
- Preserve toolkit's nullary tables through its egui adapter.
