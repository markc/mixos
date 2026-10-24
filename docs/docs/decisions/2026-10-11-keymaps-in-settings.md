# Key bindings are desktop settings

Status: accepted. Settings contract 0.4.0 (additive). Widens
[Desktop settings have one native ABP authority](2026-10-07-settings-authority.md),
whose scope was the desktop's appearance and layout.

## Context

Every egui application offers its actions as commands with optional
shortcuts ([egui applications](2026-10-09-egui-applications.md)). Before
this decision each application kept its key bindings its own way:

- dopus read an overlay file, `keymap.conf.mix`, in its config directory,
  once at start-up;
- ced had a fixed table of Notepad++-style defaults;
- BusViewer and Preferences had shortcuts written into their command lists;
- each parsed chord text in its own way (`ArrowLeft` in dopus, `Left` in
  ced).

So a person changing a key had to know which application kept it where, an
agent could not change one over the Bus, and nothing could show every
binding in one place.

## Decision

1. **The user's bindings live in settingsd.** The authored desktop profile
   gains `keymaps`: per application id, per action id, the list of chords
   the person chose. An empty list unbinds the action. An action that is
   not listed keeps the application's default.
2. **Defaults stay with the application.** An application's default chords
   ship in its code, beside the actions they run, so a new action arrives
   with its key and an application without settingsd still has its keys.
   settingsd stores only what the person changed.
3. **One chord grammar.** A chord is modifiers then one key, joined by `+`:
   `Ctrl`, `Alt` and `Shift`, in any order, then a key name. Key names are
   egui's (`ArrowLeft`, `PageDown`, `F6`, `Delete`, `Num1`), a single letter
   or digit, or one of the short forms `Up`, `Down`, `Left`, `Right`, `=`,
   `-` and `/`. `Ctrl` is the platform's command key. One stroke per chord.
   settingsd checks the grammar; the application checks the key name, and
   reports and skips one it does not know.
4. **Edited by the existing transaction.** `settings.apply` takes
   `keymaps.<app>` (the whole map, or null to drop every change for that
   application) and `keymaps.<app>.<action>` (a list of chords, or null to
   return that action to its default). Limits: 16 applications, 256 actions
   each, 4 chords per action, 32 bytes per chord.
5. **Followed live.** The summary (`settings.get {view: summary}` and the
   summary topic) carries `keymaps`, omitted while empty. Each application
   applies its own entry whenever the profile's revision changes, so a new
   binding works at once, without a restart.
6. **How an application applies them.** The toolkit's command registry keeps
   the defaults it was built with. Applying a profile entry starts from
   them:
   - each listed action takes exactly its listed chords: the first is its
     menu accelerator, the rest also run it;
   - a chord the person bound takes precedence over a default, and the
     default loses that chord;
   - an unknown action or key is skipped and reported in the application's
     log.
7. **The record format is unchanged.** `keymaps` is authored and omitted
   while empty, so every stored profile re-serialises byte for byte and its
   seal still verifies. It has no effect on the resolved design, so the
   effective digest and design revision do not move when only a binding
   changes.

## Consequences

- dopus no longer reads `keymap.conf.mix`. No such file existed on the
  running session, so nothing needed migrating.
- `<app>.actions` reports each command's effective shortcut and its further
  chords, so an agent can read what a key does before and after a change.
- A shell that decodes the full snapshot (`deny_unknown_fields`) must be
  rebuilt before a profile carries `keymaps`. The portal is one; it is
  rebuilt with this change.
- A Keyboard Shortcuts page in Preferences can follow: it reads each application's
  commands over the Bus and writes `keymaps.<app>.<action>`.

## Not covered

- The compositor's own bindings (workspace and window keys) are not an
  application's commands; they stay in the compositor for now.
- Multi-stroke chords, and keys an editor widget handles itself (Cut, Copy,
  Paste and the cursor keys in ced's editor).
