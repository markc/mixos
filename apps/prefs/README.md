# prefs

MixOS's preferences: one window with a sidebar of editors, in the spirit of
the AmigaOS Prefs drawer. The first editor is **Applications**, over the
`releases` service (`services/releasesd`): the followed apps, installed
against latest, release notes, and Install or update, Roll back, Remove
(after a confirmation), Check for updates and Update all. The manual is
[docs/prefs.md](../../docs/prefs.md).

| Path | What |
|---|---|
| `crates/preferences` | the headless engine: panels, rows, notes, one operation at a time, the `prefs.*` verbs |
| `src/commands.rs` | every action as a registered command (menus, buttons, shortcuts, `prefs.execute`) |
| `src/view.rs` | the egui rendering; returns what the person did |
| `src/shell.rs` | events in, effects out, in order; drained shutdown |
| `i18n/en/prefs.ftl` | the strings |

The Bus connection is `libs/citizen`. Run with `prefs [--noded-url URL]`; a
second launch raises the first window (`prefs.show`).

Test with `cargo test -p preferences -p prefs`. Regenerate the snapshots with
`UPDATE_SNAPSHOTS=1` and look at the images before committing them.
