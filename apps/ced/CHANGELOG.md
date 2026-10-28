<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
# Changelog

## Unreleased — 2026-10-28

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
  delivered-call timeouts remain uncertain and are never retried.

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
