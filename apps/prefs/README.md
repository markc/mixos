# prefs

MixOS's preferences: one window with an expandable, resizable navigation tree,
in the spirit of the AmigaOS Prefs drawer. **Appearance** contains Themes,
Fonts (current faces and previews) and Wallpapers. **Applications** groups
MixOS Desktop, MixOS Server and creative applications using catalogue metadata,
with catalogue-supplied display labels and Other for unclassified entries.
It works over the
`releases` service (`services/releasesd`): the followed apps, installed
against latest, release notes, and Install or update, Roll back, Remove
(after a confirmation), Check for updates and Update group. Themes is
**Appearance**, over settingsd: the session's scheme, style, mode, contrast
and window frame, previewed in the window and applied in one fenced
`settings.apply`. The manual is
[docs/prefs/README.md](../../docs/prefs/README.md).

| Path | What |
|---|---|
| `crates/preferences` | the headless engine: panels, rows, notes, one operation at a time, the `prefs.*` verbs |
| `crates/preferences/src/appearance.rs` | the Appearance panel: the look, its draft, fenced applies, receipts for lost replies |
| `src/commands.rs` | every action as a registered command (menus, buttons, shortcuts, `prefs.execute`) |
| `src/view.rs` | the egui rendering; returns what the person did |
| `src/shell.rs` | events in, effects out, in order; drained shutdown |
| `i18n/en/prefs.ftl` | the strings |

The Bus connection is `libs/citizen`. Run with `prefs [--noded-url URL]`; a
second launch raises the first window (`prefs.show`).

Test with `cargo test -p preferences -p prefs`. Regenerate the snapshots with
`UPDATE_SNAPSHOTS=1` and look at the images before committing them.
## Wallpaper

Appearance also edits backdrop's default and per-output wallpaper: theme,
solid colour or PNG/JPEG image. The native image browser runs directory I/O
off the window thread. Apply sends the complete configuration after a fresh
state check; untouched output overrides are preserved. A refused image keeps
the last good wallpaper and the local draft.

