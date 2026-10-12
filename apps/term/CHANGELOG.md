# Terminal Bus contract

## 0.4.0

The window is egui on wgpu, its grid drawn on the GPU by `term-render`.
The terminal's own verbs (`term.session`, `term.tabs`, `term.tab.*`,
`term.panes`, `term.pane.*`, `term.snapshot`, `term.scroll`, `term.type`,
`term.props.watch`, INFO and HELP) are unchanged.

Added: `term.commands` and `term.execute` (every window action by command
id), `app.describe`, the toolkit drive verbs (`term.ui.*`, `term.window`,
`term.window.state`), and `term.command.executed`, published for every
command run with how it was reached.

Program (OSC) titles now reach the tab strip without a `term.props.watch`
watcher. Right Shift with Left/Right no longer changes tab: right-hand
modifier chords are the person's own keys on MixOS.

## 0.3.5

Shared the native application bootstrap and task worker. PTY ownership
and ordered teardown stay in the terminal. The native session contract is
unchanged.

## 0.3.4

The terminal frontend and native session verbs entered the tree. The
cross-process session constants and the child-only descriptor handoff keep
their bytes.
