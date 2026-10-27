---
title: DOpus
description: MixOS's twin-pane file manager — two folder trees side by side, Places and Properties sidebars, drag and drop between panes, menus and keys for every action, and the same actions over the Bus.
---

# DOpus

DOpus is MixOS's file manager: two panes side by side, each a folder
tree, with a Places sidebar on the left and a Properties sidebar on the
right. The name is a nod to Directory Opus on the Amiga, whose twin panes
it keeps.

## The window

- **Two panes.** Each shows one folder as a tree with Name, Size and
  Modified columns; columns that do not fit are hidden, Modified first. One
  pane is active; Switch Pane (Tab) moves between them, and copy and
  move always go to the other pane. Drag the divider to change the split.
- **The path field** at the top of each pane always shows where it is. Click
  it, type a path and press Enter to go there; `~` is your home, and a file
  path opens the folder that holds it.
- **The tree.** A chevron or a double-click opens a folder in place;
  another collapses it. Double-click a file to open it in its default
  application.
- **Selecting.** Click a row to select it, Ctrl-click to add or remove one,
  Shift-click for a range. The arrow keys, Home and End move the selection.
- **Sorting.** Click a column header to sort by it, ascending first; click
  it again to reverse.
- **Places** lists Home, the filesystem root and your Desktop, Documents,
  Downloads, Music, Pictures and Videos folders, where they exist. Its
  toggle sits left of the left pane's path field.
- **Properties** shows the selected item: kind, size, dates, permissions,
  owner and path, under a preview of a file when one can be made. Its
  toggle sits right of the right pane's path field.
- **Previews** (View › Previews, per pane, on by default) are thumbnails
  from the desktop's shared thumbnail cache, made by the thumbnailers
  installed on the system (the same ones GNOME and COSMIC use). A type
  with no thumbnailer keeps its icon. Files on network, FUSE and automounted filesystems (NFS, SMB,
  sshfs, ntfs-3g and the like) keep their icons too, unless
  `remote_previews: true` is set in the configuration (below). A file
  whose filesystem stops answering keeps its icon rather than holding up
  the previews of the others.
- **Icons** show what a folder or file holds: your well-known folders have
  their own, and files are grouped by their MIME type (document, code,
  image, audio, video, archive, spreadsheet, presentation, font, disk
  image, program, shell script).
- **Drag and drop.** Drag rows from one pane onto the other, or onto a
  folder in it, and choose Move Here, Copy Here or Cancel.
- **Right-click** a row or a pane's background for the actions that apply
  there.

Deleting asks first, and naming a new folder or a rename is checked before
anything happens. Copies, moves and deletes run in the background, and the
status line says how they ended.

## Menus and keys

The menus hold every action except moving the selection (the arrow keys,
Home and End) and the quick theme keys (Ctrl+Alt+D for light or dark,
Ctrl+Alt+1 to 6 for a scheme; View › Theme has the full choice). Help ›
Keyboard Shortcuts lists every key in force.

| Menu | Action | Keys |
|---|---|---|
| File | Open | Enter |
| | Mail selection | Ctrl+M |
| | New Folder | Ctrl+Shift+N |
| | Rename | Ctrl+E |
| | Copy to Other Pane | Ctrl+C |
| | Move to Other Pane | Ctrl+X |
| | Delete | Delete |
| | Quit | Ctrl+Q |
| View | Places, Properties | Ctrl+B, Ctrl+I |
| | Hidden Files | Ctrl+H |
| | Refresh | Ctrl+R |
| | Sort by Name, Size, Modified | Ctrl+1, Ctrl+2, Ctrl+3 |
| | Theme | (this window's look; the session's by default) |
| Go | Back, Forward | Alt+ArrowLeft, Alt+ArrowRight |
| | Parent Folder | Backspace, Alt+ArrowUp |
| | Home | Alt+Home |
| | Location… | Ctrl+L |
| | Switch Pane | Tab |
| Help | Keyboard Shortcuts | Ctrl+/ |

To change a key, set it in the desktop profile: the binding applies at once,
and DOpus's defaults stay for every action you leave alone. For example,
`settings.apply` with `keymaps.dopus.nav.parent: ["Ctrl+U"]` makes Ctrl+U
the parent-folder key, an empty list leaves an action without a key, and a
reset of `keymaps.dopus` restores the defaults. See [key bindings are desktop
settings](../docs/decisions/2026-10-11-keymaps-in-settings.md).

## What it remembers

The two panes' folders, sorting and hidden-files setting, which pane was
active, the split, and the sidebars' state and widths are kept in
`config.conf.mix` in DOpus's own directory (by default
`~/.local/state/mixos/apps/dopus/config/`). `dopus --print-config` shows the
configuration it would use. One setting lives only there:
`remote_previews` (default `false`) makes previews of files on network,
FUSE and automounted filesystems too.

## For agents

DOpus answers on the Bus as `dopus` (schema `dopus.v1`).

| Command | Arguments | Answer |
|---|---|---|
| `dopus.ping`, `dopus.describe`, `dopus.info` | | liveness, the control surface, version and build |
| `dopus.state` | | both panes (path, sort, hidden files, previews, selection, rows, status) and the sidebars |
| `dopus.open` | `paths` (one per pane), or one path and `pane` | goes there |
| `dopus.action` | `id`, `pane` (`left`, `right` or `active`) | runs one action |
| `dopus.actions.list` | | every action, its keys in force and whether it can run now |
| `dopus.theme.set` | `scheme`, `mode` | this window's look |
| `dopus.quit` | | closes the window |

Every action accepts local and mesh callers, including `file.open`,
`file.new-folder`, `file.rename`, `file.copy-other-pane`,
`file.move-other-pane` and `file.delete`. `dopus.actions.list` reports the
same availability as the keyboard and menus: selection, an idle worker and
valid arguments still matter.

**Mail selection** (`dopus.action {id:"file.mail"}`) sends the current pane's
selected file paths to Email. It is available for one or more regular files,
including symlinks to files, and appears in the File and row context menus.
It launches Email through the session launcher when needed. A new message
opens with the files attached; if a compose is already open, the files join
that message instead. Email keeps paths and reads the bytes at send time.
The Bus task performs these calls without blocking the window. The status
area and action reply report whether a message opened, files were added or
Email refused; the reply's `result` contains `added`, `count` and `message`.
Headless instances support this action on their selection too.

File actions keep the normal name prompts, destructive confirmations and
drop choices. After `dopus.action {id:"file.new-folder"}`, use
`dopus.ui.type {text:"Projects"}` and `dopus.ui.click {label:"OK"}` to
answer the focused prompt. For rename, select the old text first with
`dopus.ui.key {key:"A",modifiers:["ctrl"]}`. Answer a delete confirmation
with `dopus.ui.click {label:"Yes"}` or cancel with `label:"No"`; a drop
dialog offers **Move Here**, **Copy Here** and **Cancel**. These are the
existing drive commands acting on the same dialogs a person sees. There is
no separate dialog-answer command. `dopus.ui.tree` gives the current
accessible labels when using another language. Delete keeps its normal
confirmation. Existing destinations refuse a collision; they are never
silently overwritten.

The window is drivable like every MixOS app, including its menus: see
[driving an app](../docs/drive.md) (`dopus.ui.*`, `dopus.window`). The commands are
registered in `docs/docs/spec/bus/commands.conf.mix`.

Each command run from a menu, a key, a sidebar button or an accepted
`dopus.action` is published on `dopus.action.executed`, with how it was
reached (`menu`, `shortcut`, `ui` or `bus`;
[executed commands](../docs/decisions/2026-10-11-executed-commands.md)). A refused
action is not published.

`dopus --headless` runs the core and the Bus port without a window, for
tests: the same commands, minus those that need a window. It publishes its
accepted `dopus.action` runs on `<service>.action.executed` too.
Headless instances cancel name prompts and confirmations because there is
no window to answer them; copy and move still use the real filesystem worker.

`dopus --choose-service` is a separate process registered as `files`, without
the twin-pane window or the `dopus.*` commands. It serves `files.choose`,
`files.choose.result`, `files.choose.cancel` and `files.choose.list`, the
toolkit drive commands as `files.ui.*` and `files.window*`, and discovery,
under the [file chooser contract](../docs/spec/files/README.md).
Every accepted request has a secondary native window with the file manager's
Places, path bar and single pane. Save confirms existing names before returning
URIs; the chooser never writes the chosen files. Escape, Cancel and window close
cancel the request. The last accepted folder is remembered per claimed app,
otherwise per caller, within the provider incarnation.

Drive uses the request id as `viewport`. It is required when several choosers
are open. Without a display, choose returns `UNAVAILABLE`. compd currently
exposes no portal parent-identifier resolver: windows centre on the focused
output and are raised when mapped. A parent-relative stacking relationship
cannot currently be established through that interface.
The component owns the `files@.service` session unit template; adding it does
not enable or start a service.

## Macro Bus commands

The shared [macro Bus surface](/macros/#bus-commands) uses this process's
catalogue and runner. Subscribe to `<service>.macros.finished` for completion, where `service` is
the instance’s registered Bus name (for example `term-1234` for a second Term
instance). Commands keep the app’s canonical prefix, such as `dopus.macros.run`.

| Command | Arguments | Result |
|---|---|---|
| `dopus.macros.list` | `{}` | Catalogue and diagnostics |
| `dopus.macros.reload` | `{}` | Catalogue and diagnostics |
| `dopus.macros.run` | `{id}` | Acceptance and run number |
| `dopus.macros.status` | `{}` | Current or last run |
| `dopus.macros.output` | `{}` | Retained output and dropped flag |
| `dopus.macros.stop` | `{}` | Whether termination was requested |
| `dopus.macros.new` | `{title}` | Create a user per-app macro; `{id,title,path,scanned}`, without opening an editor; scan failures add `warning` and preserve the file |

Logging: `--debug[=LEVEL]` and live Bus controls are described in
[live application logging](../docs/dev/debugging.md).
