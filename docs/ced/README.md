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
`show` (restore, raise and focus the window through compd), `state`,
`type`, `select`, `execute`, `actions`, `wait`, `layout`, `stats`,
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
- Reading the primary selection over the Bus: it is answered as empty, since
  the toolkit cannot read it on demand. (In the window, selecting text offers
  it as the primary selection and middle-click pastes it where you click.)

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

Logging: `--debug[=LEVEL]` and live Bus controls are described in
[live application logging](../docs/dev/debugging.md).

## Mail documents

Ced recognises an opening `---` block containing `mail-*` headers. Valid
[mail documents](../maildoc/README.md) open with a compact header form above
the body editor. From is a plain address; Email checks identities when you
send. Cc, Bcc and Reply-To buttons reveal optional rows. Thread headers are
read-only. Attach… chooses multiple paths; each attachment chip can be removed.
Each committed header field edit is one undo step; typing in a field coalesces
until focus leaves it or a command commits it. The last 256 header undo steps
are retained. Execution events are published on commit, not per keystroke.
Header undo history is separate
from the edit service's body history: Undo exhausts header edits first, then
undoes body edits. Redo restores header edits before body edits.

The native edit service holds the body in a scratch buffer, seeded at revision
zero, so Undo cannot remove the initial body. Ced keeps the headers with the
tab. Save and Save As assemble the document with `maildoc::write`: LF line
ends, sorted headers, omitted empty headers and canonical trailing body
whitespace. An invalid field refuses the save without touching the file.
A file changed on disk also refuses the save; use Save As or reopen it.
Saving keeps the path as opened, including relative paths and symlinks;
canonical paths are compared only to detect conflicting external edits. A
deleted file is recreated. Body dirty checks use the same LF and trailing
whitespace normalisation as the writer. A lost scratch is seeded from the
live mirror text, retaining unsaved body edits. A detached copy remains
available separately. Session mail tabs retain their path, body recovery id
and header state. Session files use private (0600) atomic replacements.
After a crash or restart they reattach the body by recovery
id, preserving unsaved edits; if editd no longer has it, Ced reopens the file.
Mail bodies belonging to session tabs are excluded from the Recovered prompt.
Save As retains the mail kind. Reload from Disk requires closing and reopening
the mail tab, so it cannot silently replace separately edited headers.

File ▸ Send saves first, then asks Email to send the absolute document path.
The tab shows Sending… until Email replies. A second Send is refused while
one is in flight. Each Email call has 120 seconds; the separate registration
wait is 10 seconds. Email's refusal retains its code and message verbatim. An
uncertain transport
outcome says the mail may have been sent; ced never retries it. Opening a
document never sends anything. Ced knows no accounts, JMAP or sending limits.

Agents use `ced.execute {id:"file.send", params:{tab:1}}`; the reply waits for
the save and Email's outcome. `ced.state` returns body text and, on mail tabs,
`mail:{headers,sending,saving,notice}`. Update one field by changing the returned
headers and calling `ced.execute {id:"mail.headers", params:{tab:1,
args:{headers:HEADERS}}}`. Thread headers cannot be changed through that action.
Send is unavailable on ordinary text and Bus-message documents (`to` plus
`command`), which remain plain text for Workshop. A refused mail candidate
also opens as unchanged text, with maildoc's exact refusal beside it for repair.
Mail tabs keep header edits in this process. Exit refuses while a mail tab
is dirty or a save/Send is in flight; save or explicitly close the tab first.
Mail scratch buffers, including those still seeding, are force-closed on exit.
Unsaved headers still block Exit while the body is seeding.
Ordinary unanswered edit requests do not consume the shutdown drain allowance;
force-closes have a separate bounded wait, preserving time for deregistration,
closing refusals and session persistence. A save waiting for body
edits holds intent only: it assembles the latest body when the edits drain.
Body typing continues while a plain Save is parked and pauses during the file
write. A parked Send immediately shows Sending… and freezes body edits, so
text typed after Send cannot join that message. Undo and Redo refuse with a
notice explaining that Save or Send is waiting for body edits while either
operation is parked.
Close and Exit cancel that intent; reconnection cancels it with a notice.
Same-frame header edits are applied before Save or Send. Header controls and
Attach are disabled during save or Send. Non-UTF-8 attachment paths are refused.
Optional header fields default when omitted by an agent.
Validation refusals use `INVALID_ARGUMENT`; busy or cancelled operations and
disk conflicts use `CONFLICT`. File I/O failures use `INTERNAL`.

Empty front matter is ordinary text. Only actual `mail-*` keys make a mail
candidate; continuation lines do not. Mail with a UTF-8 BOM stays text and
shows a notice explaining how to remove the BOM.

`ced.dialog {}` reads the open modal as `{dialog: {kind, title,
buttons: [{id, label, role}], body?, path?, status?} | null}`.
`{open: null}` closes it as Escape does; `{answer: <button id>}` presses
a listed button. No modal returns `NO_DIALOG`; a bad button returns `ARGUMENT`.
Help opens with `{open: "keys"}` or `{open: "about"}`. Recovered rows
include buffer ids: answer open or discard with `{buffer: "..."}`.
Help and recovery close with done. Path dialogs accept path; goto accepts text.
A refused dirty-mail Bus close leaves no modal behind.
