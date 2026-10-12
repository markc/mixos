# Terminal

`term` is the MixOS terminal (app id `dev.mixos.term`): tabs, split panes,
and a grid drawn on the GPU. It is an egui shell (AGENTS.md §4.1) around
two private crates:

- `crates/term-core`: the engine. PTYs, the VT grid, tabs and panes,
  selection, mouse reporting, configuration, the native session and the
  `term.*` Bus verbs.
- `crates/term-render`: the grid on the GPU. A glyph atlas and instanced
  cells drawn by an egui-wgpu paint callback per pane. The CPU raster in
  term-core is its test oracle.

The shell adds the window: the toolkit title bar and menus, the tab strip,
pane borders on whole physical pixels, the keyboard and input method, the
pointer (selection, mouse reporting, middle-click paste of the primary
selection), the clipboard, and font zoom. Every action is a command on the
toolkit registry, published on `term.command.executed`.

## Bus

The window registers `term` (or `term-<pid>` when another instance holds
it) and answers:

- the terminal's verbs, served by term-core (`HELP` lists them):
  `term.session`, `term.tabs`, `term.tab.*`, `term.panes`, `term.pane.*`,
  `term.snapshot`, `term.scroll`, `term.type`, `term.props.watch`;
- the window's own: `term.commands`, `term.execute`, `app.describe`, and the
  toolkit drive verbs (`term.ui.*`, `term.window`, `term.window.state`).

`term --headless` serves the terminal's verbs with no window.

## Configuration

`term.conf.mix`, found by the shared MixOS directory rule (`MIXOS_ETC`,
then `$MIXOS/etc`, then the user or system configuration directory). The
example is `crates/term-core/term.example.conf.mix`. `TERM_FONT_PX` and
`TERM_SPIKE_FONT` override the size and the font; `term --print-config`
prints what was resolved.

## Keys

`term --help` lists them. Tabs: Ctrl+Shift+T/W, Ctrl+PageUp/PageDown.
Panes: Ctrl+Shift+E/O split, Ctrl+Shift+X close, Ctrl+Shift+arrows,
Tab/Shift+Tab or Ctrl+Tab between split panes. Clipboard: Ctrl+Shift+C/V,
Shift+Insert. Font: Ctrl+plus/minus/0 and Ctrl+wheel. History:
Shift+PageUp/PageDown/Home/End. Right-hand modifier chords are the
person's own on MixOS and are not bound.

## Building and testing

On an authorised CBC worker at a pushed commit:

```
cargo test --locked -p term-core -p term-render -p term
cargo build --locked --profile release-fast -p term
```

`tests/window.rs` takes an offscreen snapshot of the window with two tabs
and a split; regenerate it with `UPDATE_SNAPSHOTS=1` and look at it. The
PTY tests need the Mix binary at `/opt/mixos/bin/mix`.
