---
title: Terminal
description: The MixOS terminal, with tabs, split panes and a GPU-drawn grid, driven by the session settings and fully scriptable over the Bus.
---

# Terminal

`term` is the MixOS terminal (app id `dev.mixos.term`): tabs, split panes,
and a grid drawn on the GPU. It is an egui shell (see
[AGENTS.md](https://github.com/markc/mixos/blob/main/AGENTS.md) §4.1) around
two private crates:

- `crates/term-core`: the engine. PTYs, the VT grid, tabs and panes,
  selection, mouse reporting, configuration, the native session and the
  `term.*` Bus commands.
- `crates/term-render`: the grid on the GPU. A glyph atlas and instanced
  cells drawn by an egui-wgpu paint callback per pane. The CPU raster in
  term-core is its test oracle.

The shell adds the window: the toolkit title bar and menus, the tab strip,
pane borders on whole physical pixels, the keyboard and input method, the
pointer (selection, mouse reporting, middle-click paste of the primary
selection), the clipboard, and font zoom. Every action is a command on the
toolkit registry, published on `term.action.executed`.

The source is [`apps/term`](https://github.com/markc/mixos/tree/main/apps/term).

## Bus

The window registers `term` (or `term-<pid>` when another instance holds
it) and answers:

- the terminal's commands, served by term-core (`HELP` lists them):
  `term.info`, `term.session`, `term.tabs`, `term.tab.*`, `term.panes`, `term.pane.*`,
  `term.snapshot`, `term.scroll`, `term.type`, `term.props.watch`;
- the window's own: `term.actions`, `term.execute`, `app.describe`, and the
  toolkit drive commands (`term.ui.*`, `term.window`, `term.window.state`).

`term --headless` serves the terminal's commands with no window.
`term.info` returns the prose usage and diagnostic notes. `app.describe`
keeps its richer `{schema, app_id, commands}` object on the window service.

Every command accepts local and mesh callers, including TCP callers without
a Unix principal. The allocated native-session route also serves target-bound
commands and forwards execution and isolated tasks to the pane shell.
Callers can inspect and cancel each other's operations. Targets, current
generations, request replay checks and bounded queues still apply. Input
from different callers shares the PTY write queue.

Request IDs are caller-scoped, using the broker's connection stamp for
anonymous callers. Operation IDs are minted from one counter per Term control
instance, so `(target, operation_id)` identifies one retained outcome across
all callers. Forwarded shell operations use the same counter; Term translates
their IDs when reading or cancelling the child operation.

## Configuration

The desktop profile's `terminal.*` settings, held by
[settingsd](../settingsd/README.md) and edited in [Preferences](../prefs/README.md):
font family, size and weight, padding, border width, history lines, scroll
speed, cursor shape and blink, the focus ring, copy on select, and `TERM`.
The window follows them as they change; Ctrl+plus and minus write
`terminal.font.size` back 400 ms after the last zoom key. Ctrl+0 returns to
the profile size without writing, except to correct a write already in
flight. An unsaved zoom stays pending in memory until another zoom key or a
successful settings read; a validation refusal discards it. Failures
schedule no retry timers. History lines and `TERM` apply to tabs and panes
opened afterwards. Without settingsd the defaults stand, and zoom stays the
window's own. `TERM_FONT_PX` sets the size until settingsd answers, and
`TERM_SPIKE_FONT` overrides the font file; `term --print-config` prints the
startup values.

## Keys

`term --help` lists them. Tabs: Ctrl+Shift+T/W, Ctrl+PageUp/PageDown.
Panes: Ctrl+Shift+E/O split, Ctrl+Shift+X close, Ctrl+Shift+arrows,
Tab/Shift+Tab or Ctrl+Tab between split panes. Clipboard: Ctrl+Shift+C/V,
Shift+Insert. Font: Ctrl+plus/minus/0 and Ctrl+wheel. History:
Shift+PageUp/PageDown/Home/End. Alt is Meta (ESC before the key; Alt+cursor
keys send xterm's CSI 1;3 sequences). Right-hand modifier chords are the
person's own on MixOS and are not bound.

## Building and testing

On an authorised build worker at a pushed commit:

```
cargo test --locked -p term-core -p term-render -p term
cargo build --locked --profile release-fast -p term
```

`tests/window.rs` takes an offscreen snapshot of the window with two tabs
and a split; regenerate it with `UPDATE_SNAPSHOTS=1` and look at it. The
PTY tests need the Mix binary at `<prefix>/bin/mix`.

## Macro Bus commands

The shared [macro Bus surface](/macros/#bus-commands) uses this process's
catalogue and runner. Subscribe to `<service>.macros.finished` for completion, where `service` is
the instance’s registered Bus name (for example `term-1234` for a second Term
instance). Commands keep the app’s canonical prefix, such as `term.macros.run`.

| Command | Arguments | Result |
|---|---|---|
| `term.macros.list` | `{}` | Catalogue and diagnostics |
| `term.macros.reload` | `{}` | Catalogue and diagnostics |
| `term.macros.run` | `{id}` | Acceptance and run number |
| `term.macros.status` | `{}` | Current or last run |
| `term.macros.output` | `{}` | Retained output and dropped flag |
| `term.macros.stop` | `{}` | Whether termination was requested |
| `term.macros.new` | `{title}` | Create a user per-app macro; `{id,title,path,scanned}`, without opening an editor; scan failures add `warning` and preserve the file |
