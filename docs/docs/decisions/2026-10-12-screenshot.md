---
title: Decision — screenshot
description: The screenshot and annotation app is named screenshot (app id dev.mixos.screenshot) and supersedes Cap, whose names are retired. Print Screen starts it. screenshot is an accepted binary-name collision with nwg-shell's helper.
---

# Decision — screenshot

Status: accepted 2026-10-12; amended 2026-10-13 (saved shots are named
`YYYYMMDD_HHMMSS.png`, as Spectacle can name them, instead of
`Screenshot YYYY-MM-DD HH-MM-SS.png`). Amends
[the source layout](2026-10-04-source-layout.md) (AGENTS.md §3.4).

## The decision

- **The screenshot and annotation app is `apps/screenshot`.** Its package,
  binary and directory are `screenshot`, its app id is
  `dev.mixos.screenshot` and its Bus service and verbs are `screenshot` and
  `screenshot.*`. "Screenshot" is exactly what the app does, and it is the
  word a person looks for in a launcher.
- **It supersedes Cap.** Cap was the iced screenshot app (`apps/cap`,
  `dev.mixos.cap`, verbs `cap.*`) that lived outside this tree. The egui
  port brings its behaviour across under the new name:
  - the engine moves into the `annotation` crate: the immutable capture,
    the editable objects with undo, one painter for preview and export,
    and no-clobber PNG publication;
  - the iced shell is rewritten on the toolkit, with every action a
    registry command.

  Cap's names are retired and stay reserved (§3.5): the binary `cap`, the
  app id `dev.mixos.cap` and the verbs `cap.ping`, `cap.info`,
  `cap.capture`, `cap.cancel`, `cap.open`, `cap.annotate`, `cap.move`,
  `cap.delete`, `cap.crop`, `cap.undo`, `cap.redo`, `cap.export`,
  `cap.show` and `cap.quit`. They are entered in the retired-names
  registry when it lands. None of them was ever registered here.
- **Print Screen starts it.** compd's bindings, in both profiles:
  - Print takes a region;
  - Shift+Print takes the whole output;
  - Alt+Print takes the window that had the keyboard.

  compd launches `screenshot --comp SERVICE --capture MODE`, naming its own
  Bus service (so a nested compd's key captures through that compd) and,
  for a window capture, the window that had the keyboard at the key press.
  There is one window and one headless instance per compositor:
  `screenshot` and `screenshot-headless` for `comp`, `screenshot-<comp>`
  and `screenshot-headless-<comp>` for another when the name fits in 31
  bytes, else `screenshot--<hash>` and `screenshot-headless--<hash>`; a
  window name never starts with `screenshot-headless`. If that window is already running, the new
  process hands the request to it over the Bus (`screenshot.take`) and
  exits. Unsaved annotations are never discarded by a key press: the window
  comes forward and asks first.
- **Saved shots go to the screenshots folder.** Save (Ctrl+S) writes
  `XDG_PICTURES_DIR/Screenshots/YYYYMMDD_HHMMSS.png` (local time) with no
  dialog. A taken name gets `_2`, `_3` and so on, and nothing is ever
  replaced. Every save is published on `screenshot.saved` and recorded, so
  an agent gets the latest shots from `screenshot.recent` without being
  told a path.
- **`screenshot` is an accepted name collision.**
  - Arch Linux's `extra/nwg-shell` installs `/usr/bin/screenshot`, a helper
    for that shell. It was the only owner found on 2026-10-12, checked with
    `pacman -F`.
  - In Debian trixie no package installs a `screenshot` in a binary
    directory. The name appears only inside private directories (a GIMP
    plug-in, the Qt examples), checked with the packages.debian.org
    contents search.
  - Fedora was not checked: it has no public file search that works
    without a Fedora host.
  - MixOS installs to `/opt/mixos/bin`, which comes first in its PATH, so a
    MixOS session's `screenshot` is this one. compd launches it from its
    own directory, and the desktop entry names it by absolute path.

## Why

The plain word is the right name for the plain job, and `cap` said
nothing to someone who did not already know it. The collision costs
little: nwg-shell's helper is internal to nwg-shell and is not run by name
inside a MixOS session.

Handing a shot to an agent should need no typing. A fixed folder, a name
that sorts by time, a topic on every save and a query for the latest shots
give the agent the file without the person saying where it is.

## Consequences

- AGENTS.md §3.4 lists the accepted collisions: `mix` (Elixir), `rec` and
  `play` (sox), and `screenshot` (nwg-shell).
- The `screenshot.*` verbs and its drive verbs are registered in
  `docs/docs/spec/bus/verbs.conf.mix`.
- compd's binding table gains Print, Shift+Print and Alt+Print. No bare
  F1–F12 key is bound: those belong to the person.
