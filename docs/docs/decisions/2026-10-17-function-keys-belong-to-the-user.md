---
title: Decision — the function keys belong to the user
description: No MixOS program binds F1–F12 by default, bare or with any modifier. Only the person binds them, through their keymap in the desktop profile. The toolkit's command registry refuses a default chord on one.
---

# Decision — the function keys belong to the user

Status: accepted 2026-10-17. Amends [egui applications](2026-10-09-egui-applications.md)
(AGENTS.md §4.1) and builds on
[key bindings are desktop settings](2026-10-11-keymaps-in-settings.md).

## The decision

- **F1–F12 belong to the person.** No program gives a command a default
  chord on a function key: not bare, and not with Ctrl, Alt, Shift or any
  mix of them. This covers every command registry and every default key
  table, in apps, the desktop furniture and the session.
- **The person may bind them.** A keymap entry in the desktop profile
  (`keymaps.<app>.<action>`, applied by `Registry::rebind`) may put any
  command on any function key, with any modifiers. Nothing in this decision
  narrows what a person can bind.
- **A command that wants a key gets another one.** Conventional non-F
  chords are used where they exist:
  - Keyboard Shortcuts: `Ctrl+/`;
  - Refresh: `Ctrl+R`.

  Where there is no good chord, the command keeps its menu row and has no
  default key.

### What is allowed and what is not

| | Allowed |
|---|---|
| A default chord on F1–F12, any modifiers, in any registry or key table | no |
| A further default chord (`Registry::add_shortcuts`) on F1–F12 | no |
| A person's keymap override onto F1–F12, any modifiers | yes |
| An application passing a function key through as input (a terminal sending its escape sequence to the program inside) | yes: that is not a binding |
| The keyboard layout's own keysyms (`XF86Switch_VT_n` from Ctrl+Alt+Fn) | yes: they are the layout's, and compd binds the keysym, not the F-key |

## How it is enforced

- **The toolkit's command registry refuses one.** `Registry::add` (and so
  `add_toggle` and the theme menu) and `Registry::add_shortcuts` panic when a
  default chord's key is F1–F12, whatever its modifiers. The message names
  the command id and this decision. Every app builds its registry in its
  own tests, so a violation fails `cargo test` before it can ship.
- **`Registry::rebind` is not checked.** It applies the person's bindings,
  and those may use any key. The toolkit's tests prove both sides: default
  bare, Ctrl and Shift function-key chords are refused, and an override to
  F5 binds and runs.
- **One list.** `toolkit::keymap::USER_KEYS` and `toolkit::keymap::user_only`
  name the keys. A default key table that is not a toolkit registry checks
  its chords against them in its own test.
- **AGENTS.md §4.1** states the rule.

## Why

- **The person's own bindings come first.** People put their global
  shortcuts, compositor actions, assistive tools, dictation, push-to-talk
  and macros on the function keys, because those keys are otherwise spare.
  A default an app ships on one either shadows that binding while the app
  has focus or is shadowed by it, and the person loses either way.
- **Predictability across apps.** F1, F2, F5 and the rest mean different
  things in different programs elsewhere. When no MixOS program claims
  them, a function key does in every app exactly what the person bound it
  to, or nothing.
- **A private note was not enough.** The rule lived in a note and in one
  app's test (screenshot's `no_bare_function_key_is_bound`), and new apps
  kept breaking it. A rule the registry enforces cannot be forgotten.

## Alternatives considered

**Allow the modified F-keys (Ctrl+F\*, Alt+F\*) and refuse only the bare
ones.** Rejected: nearly every modified function key is already taken
before an app starts.

- **Ctrl+Alt+F1–F12** switch virtual terminals. The keyboard layout turns
  them into the XKB `SwitchVt1`–`SwitchVt12` keysyms, compd maps those
  (`services/compd/crates/slots/src/library/input/keyboard/format/format.rs`),
  and MixOS's VT-separated sessions depend on them.
- **Ctrl+F\***: a typical stock KDE Plasma session binds Ctrl+F1–F4
  (desktops 1–4), plus Ctrl+F7, Ctrl+F9, Ctrl+F10 and Ctrl+F12 (window
  effects and the overview). Those are global grabs, so an app's default
  on one would silently never fire.
- **Alt+F\***: Alt+F1 (application menu), Alt+F3 (window operations) and
  Alt+F4 (close window) are near-universal desktop conventions.
- **Shift+F10** is the standard keyboard context-menu key, which
  keyboard-only and assistive use relies on.
- **Meta/Super+F\*** belong to the compositor.
- **Terminal programs**: F-key chords must reach TUI programs inside a
  terminal (mc, htop) unclaimed.

What is left free changes from desktop to desktop and release to release.
One simple rule, no default on a function key at all, holds everywhere; a
deny-list would be tied to one desktop's current configuration. A person
who wants any F-chord binds it in their keymap.

**A typed check** (a default-chord type that cannot hold a function key).
Rejected for now: it would change the `shortcut` field of every command in
every app, for a guard the registry already gives at start-up and in every
app's own tests.

## Migration

The existing default function-key chords on main are removed. Each command
keeps its menu row:

| App | Command | Was | Now |
|---|---|---|---|
| Monitor | `file.refresh` | F5 | Ctrl+R |
| Monitor | `help.shortcuts` | F1 | Ctrl+/ |
| BusViewer | `help.shortcuts` | F1 | Ctrl+/ |
| Preferences | `help.shortcuts` | F1 | Ctrl+/ |
| dopus | `help.shortcuts` | F1 | Ctrl+/ |
| dopus | `file.rename` | F2, Ctrl+E | Ctrl+E |
| dopus | `nav.switch-pane` | F6, Tab | Tab |
| dopus | `view.refresh` | F5, Ctrl+R | Ctrl+R |
| ced | `search.find_next` | F3 | none (Enter in the find field) |
| ced | `search.find_prev` | Shift+F3 | none |

dopus's Parent Folder gains Alt+ArrowUp beside Backspace, the usual file
manager chord. ced's list of contextual keys no longer claims F10 for the
menu bar; no code bound it. screenshot's own test is removed: the registry
now refuses what it checked, with modifiers too.

A person who wants an old key back binds it in their profile, for example
`keymaps.dopus.file.rename = ["F2", "Ctrl+E"]`.

## Consequences

- AGENTS.md §4.1 gains the rule.
- A new app that binds a function key by default fails its own registry
  tests at once, with a message pointing here.
- The affected apps' manuals list the new keys.
