---
title: ced — changelog
description: Release notes of the ced component, mirrored from apps/ced/CHANGELOG.md.
generated: by docs/_build/gen-site.mix from apps/ced/CHANGELOG.md; do not edit
---

> Mirrored from [`apps/ced/CHANGELOG.md`](https://github.com/markc/mixos/blob/main/apps/ced/CHANGELOG.md), where releases update it. The component's page is [ced](README.md). This page describes the current `main` branch of MixOS.

<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
# Changelog

## Unreleased — 2026-10-28

- Follow session font size and weight with a custom design package, keeping
  the theme file design beneath profile framing, menus and title band.
- Isolate ordinary edit requests from shutdown replies and bound force-close
  waits separately, leaving time to deregister, refuse callers and flush the
  session. Close mail scratches even when quit overtakes their seeding;
  unsaved headers still refuse Exit during seeding.
- Freeze body editing and show Sending… as soon as Send parks; parked Save
  still accepts typing. Undo and Redo explain that Save or Send is waiting for
  body edits while either is parked.
- Write mail recovery state with the shared private atomic writer, retaining
  0600 permissions and leaving stale temporary files untouched.
- Reattach session mail bodies by recovery id with their header state, or
  reopen the file if the scratch is gone. Reseed from live text while keeping
  detached copies separate. Preserve the opened path after Save and compare
  canonical forms only for conflict checks, including relative Save As.
- Keep save and parser refusal codes typed, skip unchanged header focus events,
  and defensively refuse a parked save whose tab disappeared. Verify Send
  timeout uncertainty through a delivered native Bus call.

- Park save intent until the body pipeline drains; cancel parked saves on
  close, exit or reconnection. Same-frame header edits precede Save and Send.
  Preserve current body text when recreating a lost scratch, compare canonical
  bodies and force-close clean mail scratch buffers on exit.
- Coalesce header typing into bounded field undo history and publish committed
  edits only. Refuse non-UTF-8 attachment paths before JSON conversion. Disable
  attachment selection and header commands during save or Send.
- Keep empty front matter and continuation text as text; explain BOM refusals.
  Trim sender addresses, default optional header arguments, protect relative
  paths against external edits and recreate externally deleted mail files.
- Allow 120 seconds per Email send call, separate from its registration wait;
  delivered-call timeouts remain uncertain and are never retried. Preserve
  Email's refusal code together with its message.

- Open mail documents as body-only editor tabs with shared header rows,
  optional recipients, read-only thread headers and attachment paths. Refused
  mail stays unchanged text with the parser's refusal. Bus messages stay text.
- Save and Save As validate and assemble canonical mail documents. Header
  edits are dirty and individually undoable. Native scratch initial text
  keeps the opened body out of undo history. Refuse Exit while mail tabs
  have unsaved changes or an operation in flight.
- File Send and headless `ced.execute {id:"file.send"}` save first, then
  call Email with only the document path. Preserve refusals, report uncertain
  sends, refuse duplicate in-flight sends and never retry an uncertain call.
- Use the shared 360×240 logical window minimum so quarter tiles can fit. Cover the editor at that size.
- Use the shared 360×240 logical window minimum so quarter tiles can fit. Limit optional bottom docks to a share of the available height and cover both docks with the editor at that size.
