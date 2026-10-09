---
title: Driving apps over the Bus
description: How an agent drives a native MixOS app through Bus verbs, from the widget tree and injected input to window commands and captures, with BusViewer as the worked example.
---

# Driving apps over the Bus

MixOS is agentic first: anything a person can do in a native app, an agent
can do over the Bus. Every egui app built on the shared toolkit offers two
sets of verbs:

- **Its own verbs**, which read and change its state directly, such as
  BusViewer's `busviewer.filter` or `busviewer.call`. These are the
  fastest and most precise way to work with an app.
- **The drive verbs**, which are the same in every app and work the window
  the way a hand does: read the widget tree, click, type, press keys,
  scroll, open menus, move the window and take a screenshot.

The drive verbs are named under the app's own prefix: `<app>.ui.tree`,
`<app>.ui.click`, `<app>.window` and so on. `HELP` and `app.describe` list
all of an app's verbs with their arguments, both sets included.

## Replies and refusals

A verb answers with return code 0 and a JSON body, or with return code 10
and a body naming the reason:

```json
{"error_code": "ARGUMENT", "message": "give exactly one of label and id"}
```

| `error_code` | Meaning |
|---|---|
| `ARGUMENT` | The arguments are wrong, or name something that is not there. |
| `UNKNOWN_VERB` | The app has no such verb. |
| `BUSY` | Not now: work is in flight, a dialog is open, the window is hidden, no frame has been drawn yet, or the app is closing. |
| `CANCELLED` | Input that was accepted but never delivered: the window was hidden or minimised first, or the app closed. |
| `CAPTURE` | A screenshot could not be taken or written. |
| `UNAVAILABLE` | The app does not offer the drive verbs. |

An app's own verbs may add codes of their own; BusViewer's are listed
[below](#busviewer-the-worked-example).

## The drive verbs

Coordinates and sizes are in points, egui's logical pixels: the same space
for every verb, at any display scale.

### `<app>.ui.tree`

The widget tree, from egui's AccessKit output for the last frame, depth
first from the root.

- **Arguments:** `label` (optional, a case-insensitive substring of the
  label), `exact` (optional; `true` makes `label` match the whole label,
  case and all, so "Close" does not also list "Close All") and `role`
  (optional, an AccessKit role name such as `Button`, `TextInput` or
  `MultilineTextInput`). Only matching nodes are listed.
- **Answer:** `{root, focus, nodes}`. Each node carries:
  - `id`: the node id, as **decimal text**;
  - `role`, `label`, `value` and `placeholder`;
  - `rect`: `[x0, y0, x1, y1]`;
  - `enabled`, `focused` and `selected`;
  - `checked`: `"true"`, `"false"`, `"mixed"` or null;
  - `children`: the child ids;
  - `depth`.
- **Refused** with `BUSY` until the first frame has been drawn.

Node ids are 64-bit numbers, and most JSON readers keep only 53 bits of a
number. Every id in a drive answer is therefore text, and should be passed
back as text. For a plain text label, egui puts the text in the node's
value; the drive verbs report it as the `label` too.

### `<app>.ui.click`

**Each step is one frame.** Input verbs run one pass per step and answer
a pass after their last, so their time is the window's frame interval. A
session on a background VT, or a window without focus, may be drawn only
once a second: there a `ui.click` (move, click, redraw) takes about two to
three seconds, and a `ui.type` a pass per line and per newline. Expect
that wait; it is not a hang.

Moves the pointer to a node's centre in one pass, then presses and
releases there in the next, as a quick hand does within one frame. Passes
can be far apart (a compositor may draw an unfocused window only once a
second), and egui takes a release more than 0.8 s after its press as a
long press, not a click, so the two never wait for separate passes. A
separate `ui.pointer` press and release are two passes, and may be a long
press for that reason.

- **Arguments:**
  - exactly one of `label` (exact text) or `id` (decimal text, or an exact
    integer);
  - `button`: `primary` (the default), `secondary` or `middle`;
  - `double`: true for a double-click.
- A `label` matches nodes whose label is exactly that text. If none has
  it, nodes whose placeholder (hint text) is that text match instead.
- A control (a button, link, checkbox, field, slider, menu item and so on)
  wins over text that repeats its name, such as its own tooltip or a
  label beside it.
- When several nodes still match, the click is refused with `ARGUMENT`,
  listing their ids; click one of them by id.
- **Answer:** `{target, at, menu, focused, pointer}`. `target` is the node
  that was clicked and `at` the point. The rest is the state the click
  left: the open menu (as `ui.menu` reports it), the focused node and the
  pointer position.
- **Tracing:** every input verb (`ui.click`, `ui.pointer`, `ui.scroll`,
  `ui.key`, `ui.type`) takes `trace: true`, and its answer then carries
  `passes`: one entry per pass the job ran, with every input event egui
  read in it (the platform's own and the injected ones, in order), egui's
  input `time` for the pass in seconds, and what
  egui made of them: `clicked`, `drag_started`, `dragged`, `drag_stopped`,
  `hovered` and `contains_pointer` (widget ids, the same decimal text as
  node ids), the `pointer` position, whether a button is `down`, and
  `no_longer_a_click` (the pointer has moved too far since the press for
  its release to click). Use it when a click answers but does nothing.

### `<app>.ui.pointer`

One pointer event at a point.

- **Arguments:** `x`, `y`, `action` (`move`, `press` or `release`) and
  `button` (optional, as for `ui.click`).
- **Answer:** `{menu, focused, pointer}`.

Press and release are separate requests, so an agent can drag, or open a
menu on press the way a person does.

### `<app>.ui.scroll`

Moves the pointer to `x`, `y` and scrolls there by `dx`, `dy` points, with
egui's signs: a positive `dy` moves the content down, revealing what is
above. egui animates a scroll over a few frames, so the answer may arrive
before the content has finished moving.

### `<app>.ui.key`

Presses and releases one key.

- **Arguments:** `key`, an egui key name (`Enter`, `Escape`, `Tab`,
  `ArrowDown`, `F1`, `A`, …), and `modifiers` (optional), a list of
  `ctrl`, `shift`, `alt` or `command`. Off macOS, `command` is Ctrl.
- **Answer:** `{menu, focused, pointer}`.

A key with modifiers triggers the app's shortcuts exactly as the keyboard
does.

### `<app>.ui.type`

Types `text` into the focused widget. A newline in the text presses Enter.
Focus a field first, for example with a `ui.click` on it.

### `<app>.ui.menu`

The open menu: `{menu: {menu, path, depth}}`, or `{menu: null}` when no menu
is open.

- `menu`: the open menu's title.
- `path`: the highlighted row's label at each open level, null where no row
  is highlighted.
- `depth`: the level the keyboard acts on.

### `<app>.ui.capture`

A screenshot of the window, written to a new PNG file.

- **Arguments:** `name`, optional (see the rules below).
- **Answer:** `{path, width, height}`, with the size in physical pixels.

Captures are confined to the app's own directory:

- Every capture goes to `$XDG_RUNTIME_DIR/<app>/captures/`. The app creates
  that directory, mode 0700. Each of its two levels must be a real
  directory; a symbolic link there refuses the capture.
- A caller gives at most a file name, never a path. The name must:
  - end in `.png` and be at most 128 bytes;
  - contain no `/`, `\`, NUL or `..`;
  - not start with a dot.
- Without a name, the app picks a fresh `capture-<pid>-<n>.png`.
- A name that already exists, as a file or as a link, is refused with
  `ARGUMENT`. The file is created exclusively, mode 0600, so a link planted
  while the capture waits fails the capture (`CAPTURE`) instead of being
  followed.
- Without `XDG_RUNTIME_DIR` there is nowhere to capture to, and every
  capture is refused.
- The screenshot is waited for for 10 seconds at most, then the capture is
  refused with `CAPTURE`.

### `<app>.window`

A window command, sent the way the title bar's caption buttons send it.

- **Arguments:** `action`: `minimize`, `maximize`, `restore`, `close` or
  `focus`.
- **Answer:** the window state (as `window.state` reports it) plus
  `requested`, the action.
- `maximize`, `restore` and `focus` answer two frames later, with the state
  the window then reports.
- A minimised window draws no frames, so `minimize` first settles any input
  still queued (see [Ordering](#ordering)), then answers at once.
- `close` answers at once. The app then closes as if its close button had
  been pressed, finishing work it has accepted first.

### `<app>.window.state`

`{size, outer_size, maximized, minimized, fullscreen, focused,
pixels_per_point}`, where `size` and `outer_size` are `[width, height]`.

## Ordering

The drive verbs are built so that a caller can step an app
deterministically.

- **Input runs one request at a time, in order.** Each step of a click,
  key or text is its own frame: for a click, the move, the press and the
  release. The answer comes one frame after the last step, so it reports
  what the input changed.
- **Later requests wait for earlier input.** The app holds each Bus command
  in arrival order. Further input (`ui.click`, `ui.pointer`, `ui.scroll`,
  `ui.key`, `ui.type`, `ui.capture`) joins the input queue directly. Every
  other verb waits until the input before it has been answered: reads such
  as `ui.tree`, `window` commands, and the app's own verbs. So `ui.type`
  into a field followed by an app verb that reads that field sees the
  typed text.
- **A hidden window draws no frames.** While the window is minimised or
  covered, new input is refused with `BUSY`. Input already accepted is
  settled:
  - a request that only had its redraw frame left is answered normally;
  - anything with input still to send is answered `CANCELLED`.

  A click cancelled after its press, but before its release, has its
  button released safely when the window returns. The pointer first moves
  away, so the release can never complete the click.
- **Closing answers everything.** Before an app closes its Bus connection,
  every request it has accepted gets an answer:
  - input in progress: return code 0, with `closing: true`, and
    `interrupted: true` when part of it was never sent;
  - queued input and captures: `CANCELLED`;
  - every other request, including any still arriving: `BUSY`.

## BusViewer: the worked example

BusViewer browses the services and verbs on a node and calls one verb with
a JSON body. Everything in its window is also a Bus verb.

### Reading

| Verb | Arguments | Answer |
|---|---|---|
| `busviewer.ping` | | `{schema, version}` |
| `busviewer.info` | | the whole state: connection, discovery snapshot, selection, body, last reply, status, and the arranged UI (`ui`) |
| `busviewer.tree` | | `{rows}`: the rows the tree shows, depth first, each with `key`, `kind`, `label`, `depth`, `expanded`, `selected` and its child count; verb rows add `selection`, error rows `error` |
| `busviewer.reply` | | `{text, value}`: the reply panel and the last reply |
| `busviewer.commands` | | every menu command with its label, shortcut and whether it is enabled now |
| `HELP`, `app.describe` | | every verb, with arguments |

A row's `kind` is `service`, `verb`, `error`, `peers`, `peer` or
`no_peers`. Its `key` names it in the verbs below, for example
`service:noded` or `verb:noded:noded.list`.

### Changing state

| Verb | Arguments | Notes |
|---|---|---|
| `busviewer.filter` | `text` | Filters the tree, as typing in the search field does. Answers with the rows. |
| `busviewer.expand` | `key`, `open` (bool) | Opens or closes a row that has children. |
| `busviewer.select_row` | `key` | Selects any row. A verb row also selects its verb. |
| `busviewer.select` | `service`, `verb` | Selects an advertised verb. |
| `busviewer.body` | `text` | Sets the JSON body (at most 65,536 bytes). Refused while a call is using the body. |
| `busviewer.split` | `value`, 0 to 1 | The services pane's share of the window, kept between 0.2 and 0.65. The answer reports the value in effect, to 4 places. |
| `busviewer.dialog` | `open`: `"about"`, `"shortcuts"` or null | Opens a dialog, or closes the open one. |
| `busviewer.call` | `service`, `verb` and `body`, each optional | Calls the selected verb once, or the given one; `service` and `verb` go together. The answer is the reply: `{service, verb, request_body, rc, body}`. A transport failure is never retried: it answers return code 10 with `transport_error` and `outcome_unknown`. |
| `busviewer.execute` | `id` | Runs one menu command: `file.refresh`, `file.quit`, `edit.format`, `edit.clear`, `edit.copy`, `bus.call`, `help.shortcuts` or `help.about`. |
| `busviewer.refresh` | | Discovers services again. |
| `busviewer.show` | | Restores and focuses the window. |
| `busviewer.quit` | | Closes once idle. |

While a dialog is open, the window takes no edits, so neither does the
Bus: those verbs are refused with `BUSY` until the dialog is closed.
BusViewer's own refusals are:

- `BUSY`: a refresh or call is in flight, or a dialog is open;
- `DISCONNECTED`: the node's broker is unreachable;
- `ARGUMENT` and `UNKNOWN_VERB`, as above;
- `UNKNOWN_COMMAND` and `DISABLED`, from `busviewer.execute`.

### Driving it from Mix

This script opens the Help menu, closes it again, filters the tree, and
captures the window:

```mix
-- Open BusViewer's Help menu, close it, filter the tree and capture the window.
$app = "busviewer"

fn drive($verb, $args)
  $payload = json_encode($args)
  send $app $verb body=$payload timeout=30
  if $rc != 0 then
    raise("DRIVE", $verb .. " rc=" .. to_string($rc) .. ": " .. to_string($result))
  end
  return if type($result) == "string" then json_parse($result) else $result end
end

$opened = drive("busviewer.ui.click", {label: "Help"})
print "open menu: " .. json_encode($opened.menu)
drive("busviewer.ui.key", {key: "Escape"})
$rows = drive("busviewer.filter", {text: "settings"}).rows
print to_string(len($rows)) .. " rows match"
$shot = drive("busviewer.ui.capture", {name: "settings.png"})
print "capture: " .. $shot.path
```

Each `send` waits for its answer before the next one is sent. A longer
scenario follows the same pattern: send a verb, check its return code,
print the fields that matter, and stop at the first surprise. That is how
the project's own driver checks a running BusViewer from end to end.

## For app authors

The drive verbs come from the toolkit's `drive` module, an egui plugin. It
switches AccessKit on, keeps each frame's widget tree, and injects input
through egui's input hook: the same path a person's input takes. An app
wires it up in five places:

1. `drive::install(ctx, "<app>")` once, when the window opens.
2. For each `<app>.ui.*` or `<app>.window*` command, call
   `drive::request(ctx, id, verb, args)` from inside a frame, with the
   prefix stripped. It returns either an answer to send now, or nothing,
   meaning the answer will come later.
3. `drive::logic(ctx)` once per frame, from the app's logic. It runs while
   the window is hidden too, and returns the answers to send.
4. `drive::pending(ctx)` and `drive::queues(verb)`, to hold later commands
   back while earlier input is still running.
5. `drive::finish(ctx)` before closing, sending every answer it returns
   before the Bus connection stops.

`drive::describe("<app>")` lists the verbs for `HELP` and `app.describe`.
BusViewer's shell (`apps/busviewer/src/shell.rs`) is the reference wiring.
