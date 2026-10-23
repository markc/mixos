---
title: macros
description: Shared Mix macro discovery, user storage and background execution for every MixOS app.
---

# macros

A macro is an ordinary Mix program (`.mix`) that you run from an app's
**Macros** icon or Help menu. It can use the usual Mix facilities, including
application commands and Bus verbs. Listing macros never executes them.
Macros run with your normal session authority.

## Folders and precedence

Every app scans four tiers, from lowest to highest precedence:

| Tier | Folder | Scope |
|---|---|---|
| Built-in global | `/opt/mixos/share/macros/` | Every app |
| Built-in per-app | `/opt/mixos/share/macros/<app>/` | One app |
| User global | `~/.config/mixos/macros/` | Every app |
| User per-app | `~/.config/mixos/macros/<app>/` | One app |

`<app>` is the component name, for example `ced`, `dopus` or `term`.
An absolute `MIXOS_SHARE` replaces `/opt/mixos/share`. An absolute
`XDG_CONFIG_HOME` replaces `~/.config`; otherwise the configuration root is
`$HOME/.config`. Resolution needs an absolute `HOME` and does not create
folders until you request a write or open a user folder.

A macro's id is its file stem: `hello.mix` has id `hello`. Ids contain only
lowercase ASCII letters, digits, hyphens and underscores. Dotfiles,
directories and other extensions are excluded. `.mx` documents, invalid ids
and symlinks that escape a root are reported as diagnostics. A scan is
shallow; it does not traverse another app's folder.

For the same id, **user per-app > user global > built-in per-app > built-in
global**. The catalogue lists the winning entries by id, labels each tier
and marks entries that shadow lower tiers. Missing folders are fine.

## Using the menu

The Macros icon is the first standard icon, immediately before search when
the app provides search. It opens a transient drop-down directly beneath
the icon. **New macro…** and **Open Workshop** come first, followed by this
app's macros, then global macros, in catalogue order within each group.
A dim dot marks built-ins. At most fifteen macros are shown; **More in
Workshop…** reaches the rest. Workshop is planned; both Workshop entries
currently open this app's user macro folder in Dopus, with `xdg-open` as
the fallback.

The drop-down shares the menu bar's row sizes, font, padding and frame, with
the same minimum width. Near the window's right edge it aligns to the icon's
right edge. Oversized content scrolls inside the window. An empty catalogue
shows **No macros yet**.
The New macro title field uses the same measured width and anchored placement
as the drop-down, with a full text-field height. Creation errors wrap below
the field within that frame.
Tall menus stay below the title-bar icon and scroll vertically, preserving
access to the icon. Horizontal scrolling is used only for rows wider than
the available content area.

Left release runs a macro; right release edits it. Arrows move the highlight,
Enter runs, Shift+Enter edits, and a letter jumps to the next matching title.
Run, edit, Escape and an outside click close the menu and release its keys.
One macro runs per window. A busy dot appears on the icon, and the first
menu entry becomes **Stop "Title"**. Stop kills the process group and reaps
the child in the background. Closing the menu leaves the run alone; closing
the app requests termination and background reaping.

Macro rows are dimmed while a macro runs. Ctrl+Enter in the editor still
saves, and reports that a macro is already running in its output strip.

**New macro…** asks only for a title in a one-line field at the menu anchor.
Enter creates the template in this app's user folder and opens the small
editor; Escape cancels. If creation fails, the field keeps the title and
shows the error inline so you can retry. Titles become ASCII slugs: `Tidy imports` creates
`tidy-imports.mix`, then `tidy-imports-2.mix` on a clash. A title without ASCII
letters or digits uses `macro`. Creation and copying never overwrite files.
Editing a built-in first copies it to the matching user tier.

The small editor is a non-modal floating window using the shared editor.
Click its text to edit; the app remains usable alongside it. Ctrl+S saves
and refreshes the menu catalogue. Ctrl+Enter saves then runs. **Open in Ced**
hands off larger edits, with `xdg-open` as fallback. Discovery, file storage,
launchers and notification work run on background threads.

Failed creates keep the title field and show the destination path and OS
reason inline. Failed built-in copies show the same kind of error in the
menu, without opening an editor. Editing reads an existing file and checks
its atomic-save directory first. If permissions change while editing, save
errors name the file and OS reason, and the edits remain available.
The check and save share target resolution: a symlink's target directory
is checked, rather than the directory holding the link. Read-only macro
files are refused inline, including in writable directories.

Saving keeps the document visible and editable, even during discovery.
Closing a dirty editor saves before closing; a failed save keeps the text
and error visible. Outside clicks dismiss menus with the same click-through
behaviour as the toolkit menu bar. Catalogue changes close an open macro
list so a stale highlight cannot select a different file.

Every completed run posts a notifyd toast: title and done or failed status,
plus the last few output lines. **Show output** opens that run's retained
output in the editor's bottom strip. The runner retains the last 64 KiB of
combined standard output and standard error. If notifyd is absent or sending
fails, the output strip opens automatically. A successful run without output
still reports done.

One lazy native Bus client per process registers a distinct
`<app>-macros-<pid>` service and subscribes to `notify.changed` before posting.
There are no notification subprocesses or outcome polls. Deliveries request
a repaint of the originating viewport. Each window retains at most eight
notification outputs, for up to five minutes; answering, dismissing, closing,
expiry or closing the window releases them. A Bus failure after a successful
send drops that notification's retained output without opening the editor.
Each viewport has its own menu, editor and runner state.
Closing its window also withdraws outstanding notifications with
`notify.close` and stops its runner; the background manager kills the process
group before reaping the leader. While another run is live, the output strip
shows that run; an older notification's output can be viewed when idle.

## Execution context

The runner starts `/opt/mixos/bin/mix <path>` directly, without a shell
wrapper. An absolute `MIXOS_MIX` selects another Mix binary. Its working
directory is your home, and it receives:

| Variable | Value |
|---|---|
| `MIXOS_MACRO_APP` | App component name |
| `MIXOS_MACRO_ID` | Macro file stem |
| `MIXOS_MACRO_TIER` | `builtin-global`, `builtin-app`, `user-global` or `user-app` |
| `MIXOS_MACRO_PATH` | Full macro path |

For example, save this as `~/.config/mixos/macros/hello.mix`:

```mix
-- title: Hello desktop
-- SPDX-License-Identifier: MIT OR Apache-2.0

print("Hello from " .. env("MIXOS_MACRO_APP"))
```

The first `-- title:` in the leading comment header supplies the displayed
title; otherwise the id is shown. Header reads are bounded to 8 KiB.

## Library boundaries

`libs/macros` is a core library with no egui dependency. It resolves roots,
discovers files, creates or copies user macros and owns the background
runner. `toolkit::menu::anchored` reuses menu navigation and row rendering.
`toolkit::macros::State` joins the menu and shared editor to the core, with
`toggle`, `reload` and `show` methods. Its per-window adapter keeps runtime
resources in the egui context while each app keeps the menu's open flag
in its UI state. This keeps application engines below the GUI boundary in
the application layering gate.

## Bus verbs

Email, BusViewer, Monitor, Prefs, Screenshot, Ced, Dopus and Term serve the
same macro surface under their component prefix. `list`, `status` and
`output` are read-only. `run`, `stop`, `reload` and `new` have the same
authority as executing an app command; running a macro executes user code.

| Suffix after `<app>.macros.` | Body | Reply |
|---|---|---|
| `list` | `{}` | `{macros:[{id,title,tier,path,shadows}], diagnostics:[{path,message}], scanned:true}` |
| `reload` | `{}` | Rescan and return the list shape |
| `run` | `{id}` | `{accepted:true, run, id, title}`; `NOT_FOUND` for unknown ids, `BUSY` while running |
| `status` | `{}` | `{state,run,id,title,origin,started,exit,error}` |
| `output` | `{}` | `{run,text,dropped}`; the retained 64 KiB tail |
| `stop` | `{}` | `{stopping:bool}`; termination is asynchronous |
| `new` | `{title}` | `UNAVAILABLE` until title-based creation lands |

Refusals use rc 10 and `{error_code,message}`. Bodies are limited to 8192
bytes before JSON parsing.

| Error code | Meaning |
|---|---|
| `INVALID_ARGUMENT` | Bad or oversized body, unknown fields, missing or empty id/title |
| `NOT_FOUND` | Unknown macro id |
| `BUSY` | A macro is already running |
| `UNAVAILABLE` | `new` is not yet available, or no absolute HOME/roots |
| `INTERNAL` | Scan error, worker failure or dropped reply channel |

The first list and every reload scan off the caller's thread and defer the
reply. Run loads the catalogue first if necessary and answers acceptance,
not completion. Run numbers increase only for accepted runs within the app
process. A manager-thread start refusal preserves the previous number and
identity and emits no finished event. A child spawn failure after acceptance
emits one `failed_to_start` event. States are
`idle`, `running`, `exited`, `failed` and `failed_to_start`; a signal gives
`exit:null`. Idle identity fields are null. Origins are `gui` or `bus`.

Subscribe to `<service>.macros.finished` before starting a run. Once the
manager has cleaned up and reaped the process, one event is queued with
`{run,id,title,origin,state,exit,error,dropped}`. `dropped` is the output-tail
flag; `event_dropped` counts lost Bus events. Delivery is bounded and best
effort, like command execution events. Read status and output after a gap
or reconnect. Completion uses a callback, without a polling loop.

Filesystem watching and per-app context adapters remain future work.
