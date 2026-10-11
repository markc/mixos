# ced

The MixOS Editor: a desktop editor over the `edit` Bus service. A document
lives in the service, not in the window, so several people and agents can
edit it at once; ced shows their carets, selections and changes as they
happen, and keeps typing instant by applying it locally first.

- `crates/documents`: the engine, with no UI toolkit. Open documents (one
  tab per buffer of the edit service, each a mirror with its editing model,
  highlighting and diagnostics), every action, the `ced.*` verbs and their
  waiters, reconnects and reattaches, the session.
- `src/`: the shell. The Bus thread, `ced --headless`, and the egui window
  around the shared `editor` text view.

## Running

```
ced [PATH[:LINE[:COL]]…]   open paths (in the running ced, if there is one)
ced --headless             every verb, no window
ced --service NAME         register as NAME (tests)
```

The edit service must be running. ced registers on the Bus as `ced`; a
second `ced` hands its paths to the first and exits.

## The window

- **Menus, shortcuts and Bus actions are one registry.** Every action has
  the same id in the menu bar, as a shortcut (Notepad++ conventions: see
  Help › Keyboard Shortcuts) and in `ced.action {id}`.
- **Tabs** show unsaved changes, and ◆ when another origin edited the
  document since you last looked at it.
- **The notice strip** keeps warnings, edits that could not be applied
  (re-insert them at the caret or dismiss them), and a document that lost
  its connection to the service (keep your copy or take the service's).
- **Find** (Ctrl+F, Ctrl+H) runs in the service, so it searches the shared
  text, not a local copy.
- **Markdown and commit messages wrap**; code scrolls sideways.
- **Problems** (Ctrl+Shift+M) lists the document's diagnostics.
- View › Theme picks this window's scheme, style and mode.

## Bus

`ced.*` (schema `ced.v1`): `ping`, `info`, `open`, `new`, `tabs`, `focus`,
`state`, `type`, `select`, `action`, `actions`, `wait`, `layout`, `stats`,
`diagnostics`, `problems`, plus `app.describe` and `app.quit`; and the
toolkit drive verbs (`ced.ui.*`, `ced.window`, `ced.window.state`, see
`docs/drive.md`). A Bus caller's edits carry its own origin
(`agent:ced.<caller>`), so they show as an agent's, and its undo is its
own.

## Tests

`cargo test -p documents -p ced`: the engine's unit and golden verb-fixture
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
