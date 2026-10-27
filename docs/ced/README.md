---
title: Ced
description: The MixOS Editor, a desktop text editor over the edit Bus service, where people and agents edit the same document at once and see each other's carets, selections and changes live.
---

# Ced

Ced is the MixOS Editor: a desktop editor over the `edit` Bus service
([editd](../editd/README.md)). A document lives in the service, not in the
window, so several people and agents can edit it at once. Ced shows their
carets, selections and changes as they happen, and keeps typing instant by
applying it locally first.

## Running

```
ced [PATH[:LINE[:COL]]…]   open paths (in the running ced, if there is one)
ced --headless             every command, no window
ced --service NAME         register as NAME (tests)
```

Without the edit service, Ced shows a clear waiting state within three seconds
and posts a desktop toast. The document opens automatically when the service
registers on the Bus. Ced registers on the Bus as `ced`; a
second `ced` hands its paths to the first and exits.

## The window

- **Menus, shortcuts and Bus actions are one registry.** Every action has
  the same id in the menu bar, as a shortcut (Notepad++ conventions: see
  Help › Keyboard Shortcuts) and in `ced.execute {id}`.
- **Tabs** show unsaved changes, and ◆ when another origin edited the
  document since you last looked at it.
- **The notice strip** keeps edits that could not be applied
  (re-insert them at the caret or dismiss them), and a document that lost
  its connection to the service (keep your copy or take the service's).
  Warnings and errors use desktop notification toasts, with the status line
  as a fallback if notifications are unavailable. Identical messages appear
  at most once every five seconds.
- **Find** (Ctrl+F, Ctrl+H) runs in the service, so it searches the shared
  text, not a local copy.
- **Markdown and commit messages wrap**; code scrolls sideways.
- **Problems** (Ctrl+Shift+M) lists the document's diagnostics.
- View › Theme picks this window's scheme, style and mode.

## Bus

`ced.*` (schema `ced.v1`): `ping`, `info`, `open`, `new`, `tabs`, `focus`,
`state`, `type`, `select`, `execute`, `actions`, `wait`, `layout`, `stats`,
`diagnostics`, `problems`, plus `app.describe` and `app.quit`; and the
toolkit drive commands (`ced.ui.*`, `ced.window`, `ced.window.state`; see
[driving an app](../docs/drive.md)). A Bus caller's edits carry its own
origin (`agent:ced.<caller>`), so they show as an agent's, and its undo is
its own.

`ced.actions` lists the editor's actions. Run one with
`ced.execute {id, params?}`; `params` holds optional `tab` (or `buffer`) and
`args`, for example
`{id: "file.save_as", params: {tab: 1, args: {path: "/work/notes.md"}}}`.
Without a selector, the action uses the active tab. An unknown action returns
rc 10 `UNKNOWN_ACTION`. Each run is published on `ced.action.executed`.

## Source

- `crates/documents`: the engine, with no UI toolkit. Open documents (one
  tab per buffer of the edit service, each a mirror with its editing model,
  highlighting and diagnostics), every action, the `ced.*` commands and their
  waiters, reconnects and reattaches, the session.
- `src/`: the shell. The Bus thread, `ced --headless`, and the egui window
  around the shared [editor](../editor/README.md) text view.

The source is [`apps/ced`](https://github.com/markc/mixos/tree/main/apps/ced).

## Tests

`cargo test -p documents -p ced`: the engine's unit and golden command-fixture
tests, and `tests/window.rs`, which runs the real window against the fake
edit service in-process (no Bus): typing reaches the service, and two
offscreen snapshots. Regenerate them with `UPDATE_SNAPSHOTS=1` and look at
them before committing.

## Not yet

- Macros and the Output panel's content.
- Linting Mix files on save (the Problems panel shows diagnostics other
  tools send with `ced.diagnostics`).
- Paste from the menu asks the platform for the clipboard; the primary
  selection (middle click) is the compositor's, not ced's.

## Macro Bus commands

The shared [macro Bus surface](/macros/#bus-commands) uses this process's
catalogue and runner. Subscribe to `<service>.macros.finished` for completion, where `service` is
the instance’s registered Bus name (for example `term-1234` for a second Term
instance). Commands keep the app’s canonical prefix, such as `ced.macros.run`.

| Command | Arguments | Result |
|---|---|---|
| `ced.macros.list` | `{}` | Catalogue and diagnostics |
| `ced.macros.reload` | `{}` | Catalogue and diagnostics |
| `ced.macros.run` | `{id}` | Acceptance and run number |
| `ced.macros.status` | `{}` | Current or last run |
| `ced.macros.output` | `{}` | Retained output and dropped flag |
| `ced.macros.stop` | `{}` | Whether termination was requested |
| `ced.macros.new` | `{title}` | Create a user per-app macro; `{id,title,path,scanned}`, without opening an editor; scan failures add `warning` and preserve the file |
