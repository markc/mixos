---
title: Screenshot
description: MixOS's screenshot app — Print Screen for a region, Shift+Print for the screen, Alt+Print for a window; arrows, boxes, pens, highlighter, text, numbered markers, crop and redaction that stay editable; Ctrl+S saves to Pictures/Screenshots, and agents find the newest shots on the Bus.
---

# Screenshot

Screenshot takes a picture of the screen, a window or a region you drag
out, lets you mark it up, and saves it where you and your agents can find
it. Every mark stays editable until you save: the captured pixels are never
changed, and what is saved is exactly what you see.

## Taking a screenshot

| Key | Takes |
|---|---|
| **Print** | a region: drag out a rectangle with the crosshair; a click or Enter takes the whole screen, Escape cancels |
| **Shift+Print** | the whole screen |
| **Alt+Print** | the window you were working in |

The keys work everywhere on the desktop. They open Screenshot, or hand the
shot to the one already open, and Screenshot hides itself while it takes the
picture. If the window holds annotations you have not saved, it comes
forward and asks first. Nothing you drew is ever thrown away by a key press.

In the window, **Take screenshot** (Ctrl+N) takes another shot in the mode
the toolbar shows: region, full screen or window. For a window shot, choose
the window from the list beside the mode buttons; opening the list reads
the windows again. With more than one display, the same place offers the
**output**: the one under the pointer, or a named one (also
**Capture › Output**). The **Capture** menu also has a delay (3, 5 or 10
seconds) and **Include pointer**, which is off unless you turn it on.
For a region, the status line says "Drag to select an area · Esc to
cancel" as the window hides; compd then outlines the screen until you
drag. A left drag selects, Escape or the right button cancels, and a click
without a drag keeps the selection waiting. If you select nothing before
compd's limit, the window comes back saying "Nothing was selected".
Whatever happens, a shot you asked for brings the window back on top and
focused.
Escape or **Capture › Cancel** stops a shot that is waiting out its delay.
A second Print while a shot is being taken is answered "already being
taken" in the window's status line.

## Marking it up

The tool bar on the left holds the tools. Each has a key you press with
nothing else held:

| Tool | Key | Does |
|---|---|---|
| Select and move | V | click an annotation to select it; drag it to move it |
| Crop | C | drag out the part to keep (Edit › Reset crop undoes it) |
| Arrow | A | drag from the tail to the point |
| Line | L | drag |
| Rectangle | R | drag a box |
| Ellipse | E | drag a box |
| Pen | P | draw freehand |
| Highlighter | H | draw a translucent stroke |
| Redact | X | drag a box; it is filled solid, so nothing shows through |
| Text | T | type the text in the bar at the top, then click or drag |
| Numbered marker | N | click; markers count up 1, 2, 3… |

The bar at the top sets the **ink** (red, amber, green, teal, black or
white), the **stroke width**, and for text and markers the **text size**.
Changing the ink or width while an annotation is selected changes that
annotation too. In a narrow window, the controls that don't fit are left
out of the bar; **Annotate** still has them all.

Widths and text sizes are what you see: a Medium line is the same
thickness on screen whether the shot is a 4K screen shown scaled down or a
small region shown full size. Each annotation keeps the size it was drawn
at in the image's own pixels, so zooming later makes it bigger or smaller
with the picture, and changing only the ink never changes its width. To
an agent, `width` and `size` are in the image's pixels (up to 512 and 1024).

Delete or Backspace removes the selected annotation, and Escape deselects
it. Undo is Ctrl+Z and
Redo is Ctrl+Shift+Z or Ctrl+Y. Ctrl with + and − zooms, Ctrl+0 fits the
picture to the window, the wheel scrolls, and a middle-button drag pans.

## Saving

Once there is a picture, **Copy** and **Save** buttons sit at the right
end of the bar at the top; Save's tip names the folder it saves to.

- **Save** (Ctrl+S) writes the picture straight into
  **Pictures/Screenshots**, no questions asked, as
  `20261012_193927.png` (the local date and time you saved, as
  `YYYYMMDD_HHMMSS`, the form KDE's Spectacle can be set to use). If that
  name is taken, the new file gets `_2`, `_3` and so on
  (`20261012_193927_2.png`).
  Screenshot never overwrites a file. The status line then says
  "Saved to ~/Pictures/Screenshots/…" with the file's name.
- **Save As…** (Ctrl+Shift+S) asks for a path, proposing the name Save
  would use in Pictures/Screenshots (created if missing). It refuses one
  that already exists.
- **Copy image** (Ctrl+C) puts the marked-up picture on the clipboard.

The Pictures folder is the one your desktop names
(`XDG_PICTURES_DIR` in `~/.config/user-dirs.dirs`), or `~/Pictures`.
Saved PNGs are created like any file an application saves: your umask
decides who else may read them.

The raw shot compd takes is kept, until you save, in Screenshot's own
private folder (`~/.local/state/mixos/apps/screenshot/captures`, readable
only by you). It is deleted once the shot is saved, replaced by another,
or closed, and when a capture is cancelled or fails. On start, Screenshot
also prunes that folder to the newest 20 raw shots and nothing older than
7 days. The window and headless instances share that folder.

## Handing a shot to an agent

You don't need to tell an agent where a shot is. Press Print, mark it up,
press Ctrl+S, and ask the agent to look at your latest screenshot. The
agent finds it in two ways:

- **`screenshot.recent`** lists the newest saved shots: path, size,
  whether they carry annotations, and when each was saved.
- **`screenshot.saved`** is published on every save with the same fields,
  for an agent that is listening.

```mix
-- The newest saved screenshot's path.
$body = json_encode({limit: 1})
send screenshot screenshot.recent body=$body
if $rc != 0 then raise("SCREENSHOT", to_string($result)) end
$recent = if type($result) == "string" then json_parse($result) else $result end
print $recent.shots[0].path
```

The answer names the folder too:
`{folder, shots: [{path, width, height, annotated, saved_at}]}`, newest
first.

## For agents

Screenshot answers on the Bus as `screenshot` (schema `screenshot.v1`).

| Verb | Arguments | Answer |
|---|---|---|
| `screenshot.ping`, `screenshot.info` | | liveness and the compositor served (`comp`); the document, its source and last save, the capture, the job and the UI state |
| `screenshot.capture` | `mode` (`screen`, `window`, `region`), `output`, `window` `{id, generation}`, `cursor`, `delay` (0–10) | a new document, once it is ready; any answer, refusals included, carries `restore_failed: true` if the window could not be shown again; refused while unsaved annotations exist |
| `screenshot.take` | `mode`, `window`, `comp` | as Print Screen: starts at once, or brings the window forward and asks about unsaved work at once; refused (`COMPOSITOR`) when `comp` names another compositor |
| `screenshot.cancel` | | cancels a capture during its delay |
| `screenshot.open` | `path` | opens an image; refused while unsaved annotations exist |
| `screenshot.annotate` (deprecated) | `kind`, `points`, `colour`, `width`, and `text`, `size`, `number` for text and markers | the document plus the new annotation's `id` |
| `screenshot.move`, `screenshot.delete` (deprecated) | `id` (and `dx`, `dy`) | the document |
| `screenshot.crop` (deprecated) | `crop` `{x, y, width, height}`, or null | the document |
| `screenshot.undo`, `screenshot.redo` (deprecated) | | the document, including when history is empty |
| `screenshot.save` | | saves into the screenshots folder as `YYYYMMDD_HHMMSS.png` (`_2`, `_3`… when taken); publishes `screenshot.saved` |
| `screenshot.export` | `path` | Save As to a new file |
| `screenshot.recent` | `limit` (1–100) | the newest saved shots |
| `screenshot.select` | `tool`, `mode`, `window`, `output` | the UI state |
| `screenshot.commands` | | every command with parameter schemas, labels, shortcuts and enablement |
| `screenshot.execute` | `id`, optional `params` | runs a command; answers `{executed, result, status, dialog, job}` |
| `screenshot.theme` | `scheme`, `mode` | this window's look |
| `screenshot.show`, `screenshot.quit` | | brings the window forward; closes it (refused with unsaved work) |

Coordinates are pixels of the captured image, whatever the zoom. Like any
program you run, an agent on the Bus acts as you: `screenshot.open` reads
any image you can read, and `screenshot.export` writes a new PNG wherever
you can write (never over an existing file). An agent
is refused rather than allowed to discard a person's unsaved annotations
(`UNSAVED`), and is refused while a dialog is open (`DIALOG`) or another job
runs (`BUSY`).

### Parameterised document edits

The headless `annotation` engine owns `edit.annotate` (the Shape body),
`edit.move {id, dx, dy}`, `edit.delete {id}`, `edit.crop {crop}` (null resets),
and nullary `edit.undo` / `edit.redo`. Dispatch through
`screenshot.execute {id, params}`; its reply includes the command's `result`
alongside status, dialog and job. Annotate returns document info plus the
new object's `id`; the other edits return document info. Unknown fields in
the execute body are refused with `ARGUMENT`.

Pointer strokes, text, number markers, completed object drags, crops, menus,
shortcuts and Bus calls all reach the same `command::Registry::execute`.
A drag commits on release, making one undo step and one executed event.
Delete selection is the nullary shell command `edit.delete_selected`; it
dispatches `edit.delete` with the selected id. Keymap overrides that named
the old nullary `edit.delete` must move to `edit.delete_selected`.
Uncrop dispatches `edit.crop`. The shell's dialog/job checks compose with
the engine's enablement.

The six `screenshot.annotate`, `screenshot.move`, `screenshot.delete`,
`screenshot.crop`, `screenshot.undo` and `screenshot.redo` verbs remain
deprecated aliases of execute and preserve their old reply bodies and
argument error codes at document limits. Alias undo/redo with empty history
return the document without executing or publishing an event; the command
and `screenshot.execute` remain disabled. Undo/redo use the shared
`command::History`; its byte budget counts undo and redo snapshots. Raster
workers receive a fresh history without old edits.

The window is drivable like every MixOS app, including its menus: see
[driving an app](../docs/drive.md) (`screenshot.ui.*`, `screenshot.window`). The
verbs are registered in `docs/docs/spec/bus/verbs.conf.mix`. Each command run
from a menu, a key, the toolbar or `screenshot.execute` is published on
`screenshot.command.executed` v2 with accepted `params` and returned `result`,
including successful edits reached by hand or through the deprecated aliases
([executed commands](../docs/decisions/2026-10-11-executed-commands.md)).
Edit › Delete and Edit › Uncrop each publish two executed events: first the
inner `edit.delete` / `edit.crop` with parameters, then the nullary wrapper
`edit.delete_selected` / `edit.uncrop`. The wrapper records the gesture;
the inner command records the document edit.

`screenshot --headless` runs the same verbs with no window, for agents and
tests, registered under the headless names above so Print Screen never
lands in an instance nobody can see. It never asks anything: work that would need a
question is refused.

## From the command line

```
screenshot [IMAGE]                 open the window (or bring it forward)
screenshot --capture region        take a shot, as Print does
screenshot --capture window --window ID:GEN
screenshot --headless              the Bus service alone (screenshot-headless)
screenshot --comp comp-nested      capture through another compositor
```

There is one window, and one headless instance, per compositor:

| Compositor | Window | Headless |
|---|---|---|
| `comp` (the desktop's own) | `screenshot` | `screenshot-headless` |
| another, e.g. `comp-nested` | `screenshot-comp-nested` | `screenshot-headless-comp-nested` |
| a name too long to fit | `screenshot--<8 hex>` | `screenshot-headless--<8 hex>` |

The hex is a stable hash of the compositor's name; a service name is at
most 31 bytes, and a window name never starts with `screenshot-headless`.
A second launch, or Print Screen in a compositor, hands its request to
that compositor's window, so a shot is always taken through the
compositor whose key was pressed; before handing over, the launch checks
that the running instance serves the same compositor (`comp` in
`screenshot.ping`) and refuses otherwise, and the request itself names the
compositor so the receiver refuses it too. A running window from an older
build that does not report `comp` refuses every hand-over: close it and
launch again. `--comp` and `--service` take Bus
service names: 2 to 31 of a-z, 0-9 and `-`, starting with a letter.

An agent's capture (`screenshot.capture`) hides the window and shows it
again without taking the focus or switching workspace; a person's capture
brings the window forward. Showing the window again is retried for a few
seconds; if it still fails, the shot is kept, the answer carries
`restore_failed: true` and the status line says so. Bring the window back
with Print Screen, `screenshot.show` or the taskbar; nothing restores it
later on its own.

## Source and tests

The source is
[`apps/screenshot`](https://github.com/markc/mixos/tree/main/apps/screenshot)
(app id `dev.mixos.screenshot`):

- `crates/annotation`: the headless engine (document, painter, viewport,
  screenshots folder, ledger).
- `src/`: the egui shell (`app` state and rules, `commands` registry,
  `capture` over compd, `jobs`, `view`, `shell`, `headless`). The instance
  names per compositor are in `src/names.rs`.

Test it on a build worker:

```
cargo test -p screenshot -p annotation
```

`tests/native_bus.rs` runs the headless binary against an in-process
broker and a fixture compositor; `tests/snapshot.rs` renders the window
offscreen (regenerate with `UPDATE_SNAPSHOTS=1` and look at the images);
`tests/registry.rs` checks the verb registry.

## Notes

- Copy image goes through the desktop's X11 clipboard (XWayland), so
  native Wayland applications may not see it yet. Save, then paste the
  file, is the reliable path for now.
- Screenshot replaces Cap, the earlier screenshot app; see the
  [decision](../docs/decisions/2026-10-12-screenshot.md).

## Changes from Cap

- Print, Shift+Print and Alt+Print take shots from anywhere.
- Ctrl+S saves straight into Pictures/Screenshots; agents find saves with
  `screenshot.recent` and `screenshot.saved`.
- Stroke width and text size are steps (thin to heavy, small to huge)
  rather than free values; numbered markers are discs.
- Open and Save As take a typed path rather than a file browser.
- The pointer is left out of shots unless you include it, for people and
  agents alike.
- A region selection may take up to 70 seconds (Cap allowed 38).
- Raw shots are deleted when no longer needed, and the folder is pruned.

## Macro Bus verbs

The shared [macro Bus surface](/macros/#bus-verbs) uses this process's
catalogue and runner. Subscribe to `<service>.macros.finished` for completion, where `service` is
the instance’s registered Bus name (for example `term-1234` for a second Term
instance). Verbs keep the app’s canonical prefix, such as `screenshot.macros.run`.

| Verb | Arguments | Result |
|---|---|---|
| `screenshot.macros.list` | `{}` | Catalogue and diagnostics |
| `screenshot.macros.reload` | `{}` | Catalogue and diagnostics |
| `screenshot.macros.run` | `{id}` | Acceptance and run number |
| `screenshot.macros.status` | `{}` | Current or last run |
| `screenshot.macros.output` | `{}` | Retained output and dropped flag |
| `screenshot.macros.stop` | `{}` | Whether termination was requested |
| `screenshot.macros.new` | `{title}` | Create a user per-app macro; `{id,title,path,scanned}`, without opening an editor; scan failures add `warning` and preserve the file |
