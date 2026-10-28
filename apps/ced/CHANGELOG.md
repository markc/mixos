<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
# Changelog

## Unreleased — 2026-10-28

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
