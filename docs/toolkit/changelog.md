---
title: toolkit — changelog
description: Release notes of the toolkit component, mirrored from libs/toolkit/CHANGELOG.md.
generated: by docs/_build/gen-site.mix from libs/toolkit/CHANGELOG.md; do not edit
---

> Mirrored from [`libs/toolkit/CHANGELOG.md`](https://github.com/markc/mixos/blob/main/libs/toolkit/CHANGELOG.md), where releases update it. The component's page is [toolkit](README.md). This page describes the current `main` branch of MixOS.

<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
# Changelog

## Unreleased — 2026-10-28

- Follow the summary custom-source flag explicitly, retaining the file design
  and its overlay while applying profile framing, menus, band and fonts.
- Bundle the unmodified Inter 4.1 Bold (700) face under the existing SIL OFL
  licence. Interface weights 300–700 select distinct faces; heading offsets
  clamp to that range. Headless coverage tests require each weight to draw
  more ink, and size tests check proportional styles without changing mono.
- Keep session font overrides when an app falls back from a custom or unknown
  design to its installed theme.
- Scale native button, link, tab, panel, dialog, title and value text metrics
  with the interface size. Apply the ratio once in chrome and pair text styles;
  keep independently sized monospace content and spacing unchanged.
- Size numeric fields for their rendered number, unit and padding at changed
  interface sizes. Preserve authored geometry and snapshots at scale 1.0.
- Add `header_form`: measured label columns, caller-drawn rows, recipient/text
  fields and removable path chips shared by Email and ced. Caller-provided
  IDs keep field state scoped to each tab and viewport.

- Allow a viewport to require its own complete client frame independently of
  the session decoration preference and later theme changes. Each viewport
  remembers its own decoration request, so a client-framed child beside a
  server-decorated root no longer re-asks and repaints both every frame.

- Scope menu models, navigation and search state to each viewport. Every
  drive command honours its named `viewport`, including menu list/open and
  root aliases. Saved-window capture workers use distinct per-window identities.
- Make chooser outcomes cloneable so deferred viewports can queue them for
  their owning engine without running file operations in rendering copies.
  Directory and path workers wake the viewport that owns their chooser.

- Refuse captures with `NOT_SHOWN` after 1.5 seconds without a viewport UI
  pass, or after emitting a screenshot whose GPU copy is never submitted.
  Collect completed GPU readbacks first and discard late callbacks by
  screenshot attempt tag.
  A renderer receipt protects submitted GPU copies until the capture deadline.
- Default and cap capture deadlines at five seconds from acceptance,
  including queue time, so refusals precede a ten-second caller timeout.

## Unreleased — 2026-10-27

- Use the shared design title band layout for menus, title, Action Bar and
  captions. Follow all four group alignments and preserve the Action Bar tail
  as app icons give way. Registered SSD apps expose revisioned icons through
  `ui.titlebar.form`; unregistered or CSD windows keep their own bar. Shared
  Lucide sources now live in `icons`, with the same egui image loader.
- Add single-file and save modes to the file chooser. Save has a file-name
  field, caller-supplied confirm label and replacement question; path probes
  stay on workers. Expose the chosen folder and XDG Documents resolution.
- Serve the citizen `.debug.recent` query with a bounded, optional event ring;
  retain viewport lifecycle diagnostics without copying the app name.
- Expose the shared debug logging helper and record native window and
  viewport lifecycle transitions, without logging idle UI passes.

- Add a reusable multi-file chooser with asynchronous directory/path probes,
  bounded listings and outstanding workers, keyboard navigation, hidden files,
  Ctrl/Shift selection, stable accessible labels and stacked modal behaviour.
- Let registries mark dialog-only menus and render selected menus inside a Ui.
  Covered bars leave the active dialog's menu and drive state alone. Dialogs
  can suspend their controls and keyboard handling while a child is open.
- The chooser shows sizes as people read them (`text::human_size`, shared
  with Email) in aligned size and date columns.

- Keep `drive::input_request` available when local and injected input share
  a pass. Expose `drive::input_mixed` so apps can record that ambiguity without
  losing the drive caller or changing input timing.

## Unreleased — 2026-10-24

- Let sliders reserve modified arrow keys for the person's bindings.
- Give the New macro title field the drop-down's measured width, anchored
  placement and a full text-field height. Wrap inline errors within that frame.
- Cover title entry at scale 2.5, including wider catalogues and a snapshot.
- Share atomic-save target resolution with the editor and probe that exact
  directory, including through symlinks. Refuse read-only macro files inline.
- Keep tall macro menus below their icon and scroll vertically; reserve the
  themed scrollbar gutter without adding an unnecessary horizontal scrollbar.
- Pin the empty menu's final row after separator cleanup.

## Unreleased — 2026-10-23

- Give icon-anchored menus the menu bar's minimum width and frame metrics;
  right-align at the icon near window edges and constrain oversized content.
- Explain empty macro catalogues with a disabled row.
- Keep failed macro creates and copies in the menu with path-specific errors.
  Check existing files' save directories before editing and retain dirty edits
  with path-specific save errors if permissions later change.
- Focus the macro title only after its visible widget is drawn, and release
  focus before disabling it for creation, keeping AccessKit focus valid.
- Save macro editor snapshots independently of discovery, keep the document
  visible during writes, and save dirty edits before closing the editor.
- Wake idle macro windows only for live work or native Bus deliveries.
- Invalidate cached picks on catalogue changes and take title-field keys
  before app content; match menu-bar outside-click dismissal.
- Withdraw notifications on window close and prioritise live run output.
- Cover menu/app synchronisation and runner teardown with hermetic fixtures.

- Use one lazy native Bus client and `notify.changed` subscription per
  process, with bounded output retention and repaint-driven deliveries.
- Dim macro rows during a run; Ctrl+Enter still saves and reports an active run.
- Keep the title field and show an inline error when creating a macro fails.
- Scope macro state, widget IDs and anchored-menu input to each viewport,
  releasing notification output when a viewport closes.

- Replace the modal Macros panel with an icon-anchored menu, title-only
  creation and a non-modal shared editor with save/run shortcuts.
- Report completed macros through notifyd, retaining output per notification
  and opening the output strip if notifications are unavailable.
- Reuse menu navigation and drawing for anchored menus, including secondary
  release, Shift+Enter editing and title-initial navigation.
- Accept the shared editor's palette role interface without a dependency cycle.

## Unreleased — 2026-10-21

- Publish command contract 0.2.0 executed bodies (v2, bounded params and result).
- Map authorisation refusals to legacy `CommandError::Denied`.
- Report stored shortcut and `more` chords in the keymaps grammar, with
  Ctrl, Alt, Shift modifier order, through both command adapters.
