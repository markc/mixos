<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
# Changelog

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
