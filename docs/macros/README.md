---
title: macros
description: Shared Mix macro discovery, user storage and background execution for every MixOS app.
---

# macros

A macro is an ordinary Mix program (`.mix`) that you run from an app's
**Macros** icon or Help menu. It can use the usual Mix facilities, including
application commands and Bus commands. Listing macros never executes them.
Macros run with your normal session authority.

## Folders and precedence

Every app scans only its two per-app tiers, from lowest to highest precedence:

| Tier | Folder | Scope |
|---|---|---|
| Built-in per-app | `<prefix>/share/macros/<app>/` | One app |
| User per-app | `~/.config/mixos/macros/<app>/` | One app |

`<app>` is the component name, for example `ced`, `dopus` or `term`.
An absolute `MIXOS_SHARE` replaces `<prefix>/share`. An absolute
`XDG_CONFIG_HOME` replaces `~/.config`; otherwise the configuration root is
`$HOME/.config`. Resolution needs an absolute `HOME` and does not create
folders until you request a write or open a user folder.

A macro's id is its file stem: `hello.mix` has id `hello`. Ids contain only
lowercase ASCII letters, digits, hyphens and underscores. Dotfiles,
directories and other extensions are excluded. `.mx` documents, invalid ids
and macro symlinks are reported as diagnostics. A scan is
shallow; it does not traverse another app's folder.

For the same id, **user per-app > built-in per-app**. Global macros at the
roots belong to Workshop; apps neither list nor run them, and globals never
shadow app macros. `Catalogue::scan_globals` provides Workshop's catalogue. The catalogue lists the winning entries by id, labels each tier
and marks entries that shadow lower tiers. Missing folders are fine.

## Using the menu

The Macros icon is the first standard icon, immediately before search when
the app provides search. It opens a transient drop-down directly beneath
the icon. **New macro…** and **Open Workshop** come first, followed by this
app's macros in catalogue order.
A dim dot marks built-ins. At most fifteen macros are shown; **More in
Workshop…** reaches the rest. Workshop is planned; both Workshop entries
currently create and open the global user macro folder in Dopus, with `xdg-open` as
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
One macro runs per application process, shared by GUI and Bus callers across all windows. This supersedes the original menu decision’s per-window execution rule. A busy dot appears on the icon, and the first
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
It is a decorated, resizable OS window, titled with the macro name.
Click its text to edit; the app remains usable alongside it. **Save** or Ctrl+S saves
and refreshes the menu catalogue. **Run** or Ctrl+Enter saves then runs.
An asterisk and an unsaved label mark edits. Escape in the focused editor
or its window close button closes only the editor. **Open in Ced**
hands off larger edits, with `xdg-open` as fallback. Discovery, file storage,
launchers and notification work run on background threads.

Failed creates keep the title field and show the destination path and OS
reason inline. Failed built-in copies show the same kind of error in the
menu, without opening an editor. Editing reads an existing file and checks
its atomic-save directory first. If permissions change while editing, save
errors name the file and OS reason, and the edits remain available.
Macro paths refuse symlinks, including links within the root. Reads and saves
walk from an opened root directory and never follow a replaced path. Read-only macro
files are refused inline, including in writable directories.

Saving keeps the document visible and editable, even during discovery.
Closing a dirty editor saves before closing; a failed save keeps the text
and error visible. Outside clicks dismiss menus with the same click-through
behaviour as the toolkit menu bar. Catalogue changes close an open macro
list so a stale highlight cannot select a different file.

Every completed GUI run posts a plain notifyd toast with no actions: title
and done or failed status, plus the last few output lines. Run errors also
post toasts. The runner retains the last 64 KiB of combined output, available
through the output command and the editor's output strip while it is open.
If notification delivery fails, an open editor shows the output; otherwise
the app logs the failure. Results and errors never open the editor. Only
right-click, Shift+Enter or successful New macro creation opens it.

One lazy native Bus client per process sends results without outcome
subscriptions or polling. Each originating window owns its editor and
output strip. Closing that window stops its GUI run through `stop_if_origin`;
closing another window leaves the run alone.

Drive commands accept `viewport: "macro-editor"` to address the open editor,
including `ui.tree`, `ui.click`, `ui.key`, `ui.type` and `ui.capture`. Omitting
the field addresses the app window. A closed editor returns `NOT_FOUND`.

## Execution context

The runner starts `<prefix>/bin/mix <path>` directly, without a shell
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

`Library::scan` scans the global tiers and uses `Roots::apps()` with the same
per-app scan to discover every app's macros. Global user macros live directly
in `~/.config/mixos/macros/`; built-ins live directly in
`<prefix>/share/macros/`. Workshop's Session runs and creates global macros.
App ids are scoped to their catalogue, so identical ids in different apps
remain separate.

A global macro can declare `-- target: any` or `-- target: ced, dopus` in
its leading comment header. `Entry.targets` is absent, `Targets::Any`, or
`Targets::List`. Empty, duplicate or malformed target headers produce a
diagnostic and exclude the file. `Session::run_with(id, origin, target)`
accepts an optional app component name and sets `MIXOS_MACRO_TARGET` only
for an allowed choice. A disallowed choice returns `INVALID_ARGUMENT`
before accepting a run. Without a choice the variable is removed from the
child environment, including any inherited value. A header-free macro runs
without a target and names its own services in its source.

Native Mix sends from the runner carry a bounded `macro-origin` header such
as `macro:ced:tidy-imports`, derived from the runner's app/id context. Workshop
uses it to filter observed requests during a run. It is telemetry and conveys
no additional authority.

`Roots::read`, `write`, `remove` and `move_to` share root confinement and
regular-file checks. Writes replace the directory entry atomically through an
opened parent descriptor. Moves use an atomic no-replace rename, without an
empty destination reservation. Built-in mutations
and destination clashes are refused. Moving preserves the id; rescan both
catalogues afterwards to recompute shadows.

## Library boundaries

`libs/macros` is a core library with no egui dependency. It resolves roots,
discovers files, creates or copies user macros and owns the background
runner. `toolkit::menu::anchored` reuses menu navigation and row rendering.
`toolkit::macros::State` joins the menu and shared editor to the core, with
`toggle`, `reload` and `show` methods. Its per-window adapter keeps runtime
resources in the egui context while each app keeps the menu's open flag
in its UI state. This keeps application engines below the GUI boundary in
the application layering gate.

## Bus commands

Email, Workshop, Monitor, Prefs, Screenshot, Ced, Dopus and Term serve the
same macro surface under their component prefix. `list`, `status` and
`output` are read-only. `run`, `stop` and `reload` have the same
authority as executing an app command; running a macro executes user code.

| Suffix after `<app>.macros.` | Body | Reply |
|---|---|---|
| `list` | `{}` | `{macros:[{id,title,tier,path,shadows}], diagnostics:[{path,message}], scanned:true}` |
| `reload` | `{}` | Rescan and return the list shape |
| `run` | `{id,target?}` | `{accepted:true, run, id, title}`; `NOT_FOUND` for unknown ids, `BUSY` while running; target must be permitted by the header |
| `status` | `{}` | `{state,run,id,title,origin,started,exit,error}` |
| `output` | `{}` | `{run,text,dropped}`; the retained 64 KiB tail |
| `stop` | `{}` | `{stopping:bool}`; termination is asynchronous |
| `new` | `{title}` | Creates in the user per-app tier; replies `{id, title, path, scanned}` without opening an editor; if the rescan fails, `scanned:false` and `warning` report it while preserving the created file |
| `get` | `{id}` | `{id,tier,path,text}`, bounded to 256 KiB |
| `put` | `{id,text}` | Atomically save a user macro and refresh the Session |
| `remove` | `{id}` | Remove a user macro and refresh the Session |
| `move` | `{id,tier,destination?}` | Move between user tiers without overwriting; destination names the app for `user-app` |
| `copy` | `{id}` | Copy a built-in into the corresponding user tier |

Refusals use rc 10 and `{error_code,message}`. Bodies are limited to 8192
bytes before JSON parsing, except `put`: its envelope is capped at 1544 KiB
and its text at 256 KiB. Workshop creates and runs globals; its `new` creates
directly in the global user folder. Management replies follow the owning
Session's rescan, so an app's menu reflects the mutation immediately.

| Error code | Meaning |
|---|---|
| `INVALID_ARGUMENT` | Bad or oversized body, unknown fields, missing or empty id/title, or an invalid single-line title |
| `NOT_FOUND` | Unknown macro id |
| `BUSY` | A macro is already running |
| `UNAVAILABLE` | No absolute HOME/roots |
| `DENIED` | Built-in mutation or an unsafe/non-editable storage path |
| `ALREADY_EXISTS` | A move or copy would overwrite a destination |
| `RESOURCE_EXHAUSTED` | A get exceeds the bounded text read |
| `INTERNAL` | Create/scan I/O error, worker failure or dropped reply channel |

Creation (including its rescan), the first list and every reload run off the caller's thread and defer the
reply. Run loads the catalogue first if necessary and answers acceptance,
not completion. Run numbers increase only for accepted runs within the app
process. A manager-thread start refusal preserves the previous number and
identity and emits no finished event. A child spawn failure after acceptance
emits one `failed_to_start` event. States are
`idle`, `running`, `exited`, `failed` and `failed_to_start`; a signal gives
`exit:null`. Idle identity fields are null. Origins are `gui:<window-id>` or `bus`.

Subscribe to `<service>.macros.finished` before starting a run. Once the
manager has cleaned up and reaped the process, one event is queued with
`{run,id,title,origin,started,state,exit,error,text,dropped}`. `dropped` is the output-tail
flag; `event_dropped` counts lost Bus events. Delivery is bounded and best
effort, like command execution events. Read status and output after a gap
or reconnect. Completion uses a callback, without a polling loop.

Filesystem watching and per-app context adapters remain future work.

GUI runs use `origin: gui:<window-id>`, with a process-local numeric window id.
Bus runs use `origin: bus`. Both publish `<service>.macros.finished`, where `service` is the instance’s
registered Bus name (for example `term-1234` for a second Term instance).
Commands retain the app’s canonical prefix, such as `term.macros.run`; only the
starting GUI window posts a toast. All windows show the shared busy dot and
Stop title. Closing the starting window stops its GUI run and removes its
finish subscription; closing another window does not stop the run.

Session finish subscribers register with `add_finish_hook` and unregister
with `remove_finish_hook`. They run once in registration order, outside the
session lock. Window callbacks use weak lifetime guards for in-flight delivery.
The completion body includes the retained output `text`, captured before a
subsequent run can reset it. Weak change observers wake every live viewport
for Bus starts, catalogue changes and completion without idle polling.

Every macro command accepts every caller, local or mesh. Writes (`put`,
`remove`, `move`, `copy`, `new`) include global and cross-app operations.
Workshop’s forwarding commands and action registry use the same open path.
Built-in files stay read-only, and storage paths remain confined to the
macro roots. Argument validation, request limits and single-flight execution
still apply. Workshop can create and immediately open a macro with
`workshop.macros.new {app:"ced",title:"Tidy imports"}`; use `app:"global"`
for the global tier.
