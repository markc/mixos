# File chooser contract: `files.choose`

Status: draft, version 0.1.0, for review before any code. Nothing here is
served yet.

MixOS has one system file chooser. A person picks files or a save location
in it; it never opens, reads or writes the files. It is reached two ways:

- **Over the Bus**, with `files.choose`. Apps and agents call it directly; an
  agent uses it to ask a person to pick a file.
- **Over D-Bus**, through `org.freedesktop.portal.FileChooser`, which
  `portald` serves and forwards to `files.choose`. Every app that asks the
  desktop portal for a dialog uses this route, whatever its toolkit (rfd,
  GTK, Qt, Electron and Firefox).

```text
app --D-Bus FileChooser.OpenFile--> portald --Bus files.choose--> chooser window
agent --Bus files.choose--------------------------^                  |
          <---- outcome on the files.chosen topic, or files.choose.result
```

## Provider

The Bus service `files` is served by DOpus in chooser mode,
`dopus --choose-service`, a session unit started with the desk. It has no main
window. Each accepted `files.choose` opens one chooser window: a single DOpus
pane with Places, a path bar, a file-name field when saving, a filter menu,
the caller's choices, and Accept and Cancel. Several requests open several
windows. Requests keep no state from one to the next, apart from the
Places, bookmarks and recent folders that the file manager also uses.

## Commands

Owner `dopus`. They enter `docs/docs/spec/bus/commands.conf.mix` (status
draft; classes as given below) with the code that serves them, because
DOpus's registry test requires the registry to list exactly what DOpus
serves.

### `files.choose` (intent)

Opens a chooser and answers straight away; the person's answer comes later
(see [Outcome](#outcome)). Body:

| Field | Type | Default | Meaning |
|---|---|---|---|
| `mode` | `open`, `save` or `save_files` | `open` | What the person is choosing. |
| `title` | string, 1 to 256 bytes | by mode | The window title. |
| `accept_label` | string, 1 to 64 bytes | by mode | The Accept button's label. Mnemonic underscores are removed. |
| `multiple` | bool | false | `open` only: more than one entry may be chosen. |
| `directory` | bool | false | `open` only: choose folders, not files. |
| `filters` | list of filter, at most 64 | none | See [Filters](#filters). |
| `current_filter` | filter | first filter | The filter selected at first. It need not be in `filters`. |
| `choices` | list of choice, at most 16 | none | See [Choices](#choices). |
| `current_folder` | location | last folder used, else home | The folder shown first. |
| `current_name` | string, 1 to 255 bytes, no `/` | none | `save` only: the suggested file name. |
| `current_file` | location | none | `save` only: an existing file to save over; its folder and name are shown. Takes precedence over `current_folder` and `current_name`. |
| `files` | list of string, 1 to 1,000 | none | `save_files` only, and required: the names of the files about to be saved into the chosen folder. |
| `parent_window` | string | `""` | The window the chooser belongs to. See [Placement](#placement). |
| `token` | 1 to 64 of `A-Z a-z 0-9 _` | generated | Names the request; see below. |

A **location** is either an absolute path written as a UTF-8 string, or a
`file://` URI (RFC 8089) whose percent-encoding carries the exact bytes of a
path that is not UTF-8. Nothing else is accepted.

Unknown fields are ignored. A known field of the wrong type or out of range,
a field used in the wrong mode, or `save_files` without `files` is refused
with `ARGUMENT`.

Reply, rc 0:

```json
{"request": "c42_open_1", "topic": "files.chosen"}
```

`request` is `<caller>_<token>`, where `<caller>` is the broker's connection
number for the caller. A `token` the caller has already used for a request
that is still open is refused with `ARGUMENT`; a caller who wants to know the
request name before calling (to subscribe first, for example) sends its own
`token`.

`BUSY` is returned when 8 choosers are already open on the desk, or 2 for this
caller's connection. `UNAVAILABLE` is returned when the chooser window cannot
be opened (no display, for example). Neither opens a window.

### `files.choose.result` (query)

Body `{request}`. Answers with the request's state:

- `{state: "open"}`: the person has not answered;
- `{state: "done", outcome: {...}}`: the outcome, exactly as published;
- `ARGUMENT` when the request is unknown, or its outcome has expired.

An outcome is kept for 10 minutes after it is decided, then removed. Reading
it does not remove it. This command is for a caller that missed the
publication; a caller that is subscribed has no need of it.

### `files.choose.cancel` (intent)

Body `{request}`. Closes an open chooser. The outcome is response 1 with
`reason: "caller"`. A request that is already done is left as it is and
answers rc 0. An unknown request is `ARGUMENT`.

### `files.choose.list` (query)

Body none. Answers `{requests: [{request, mode, title, caller, opened_ms}]}`
for every open chooser, oldest first.

## Outcome

When a request ends, one outcome is published once on topic `files.chosen`
and kept for `files.choose.result`:

```json
{
  "request": "c42_open_1",
  "response": 0,
  "uris": ["file:///home/user/Pictures/beach.png"],
  "current_filter": {"name": "Images", "patterns": ["*.png", "*.jpg"], "mime_types": []},
  "choices": {"encoding": "utf8", "reencode": "true"},
  "reason": "chosen"
}
```

| `response` | `reason` | When |
|---|---|---|
| 0 | `chosen` | The person accepted. |
| 1 | `cancelled` | The person pressed Cancel or Escape, or closed the window. |
| 1 | `caller` | `files.choose.cancel`, or the caller's broker connection closed. |
| 2 | `failed` | The chooser failed after it opened. `message` says why. |

Response codes are the portal's, so portald passes them on unchanged.

- `uris` are `file://` URIs, percent-encoded from the exact bytes of each path.
  On `open`, there is one unless `multiple` is set, never none. On `save`,
  there is exactly one. On `save_files`, there is one per name in `files`, in
  the same order. Each one is inside the chosen folder, with the person's
  answer to any name that already exists there.
- `current_filter` is the filter selected when the person accepted; it is
  absent when there were no filters.
- `choices` maps each choice's `id` to the option chosen; a choice with no
  options has `"true"` or `"false"`.
- `uris`, `current_filter` and `choices` are present only when `response` is 0.

The topic is published by `files` alone. Every subscriber sees every outcome;
the Bus is open (`docs/docs/decisions/2026-10-26-open-bus-for-agents.md`),
and an outcome carries paths, never file contents.

## Filters

A filter is `{name, patterns, mime_types}`: a label of 1 to 128 bytes, and
together 1 to 64 entries in two lists of strings:

- `patterns`: shell globs (`*`, `?`, `[...]`), matched against the file name,
  case sensitive;
- `mime_types`: MIME types or `type/*`, matched by the type the shared MIME
  database gives the file.

Folders are always shown, so the person can move between them. When `save`
is used with a filter whose only pattern is `*.ext`, a typed name without an
extension gets that one.

## Choices

A choice is `{id, label, options, default}`:

- `id` and `label` are strings of 1 to 64 and 1 to 128 bytes;
- `options` is a list of up to 32 `{id, label}`. When it is empty the choice
  is a check box, and `default` is `"true"` or `"false"`;
- `default` is one of the option ids; it is the first option when absent.

They are shown under the file list, in order.

## Behaviour

- **No timeout.** A chooser waits for the person as long as it takes.
- **Save.** When the chosen file exists, the person confirms replacing it
  before Accept succeeds. `save_files` does the same once for all the names
  that exist. The chooser never creates, truncates or replaces anything; the
  caller does that with the URIs it receives.
- **Who is asking.** When the caller is not `portald`, the window says which
  caller asked (`citizen::caller_key`), so a person can tell an agent's
  request from an app's.
- **Drive.** Each chooser window answers the toolkit drive commands
  (`docs/docs/drive.md`), so it can be tested and answered over the Bus.
  The drive command names for several windows in one process are not yet
  settled (open question 2), so none are registered yet.
- **Caller leaves.** When the caller's broker connection closes, its open
  choosers close with response 1, `reason: "caller"`.
- **Provider stops.** If `files` stops, every open request is lost. A caller
  that sees the service unregister treats its open requests as response 2.

## Placement

`parent_window` is the portal's window identifier: `""`, `wayland:<handle>`
(an exported xdg-foreign handle) or `x11:<hex id>`. In this version the
chooser opens centred on the output of the parent window when compd can
resolve the identifier, otherwise on the focused output, and stays above the
parent. It is not modal to the parent; that needs xdg-foreign import and
xdg-dialog-v1 support in compd, a later step.

## D-Bus: `org.freedesktop.portal.FileChooser`

portald serves the interface at version 4 on the object it already serves
(see `../portal/README.md`), for trusted unsandboxed callers in the session.
`OpenFile(s parent_window, s title, a{sv} options) -> o`, `SaveFile` with the
same arguments, and `SaveFiles` with the same arguments all return a Request
object straight away, at
`/org/freedesktop/portal/desktop/request/<sender>/<handle_token>`, built as
for ScreenCast. Then portald calls `files.choose` with the mapped body and
`token` set to the handle token.

| Portal | `files.choose` |
|---|---|
| method `OpenFile` / `SaveFile` / `SaveFiles` | `mode` `open` / `save` / `save_files` |
| `parent_window` argument | `parent_window` |
| `title` argument | `title` (the mode's default when empty) |
| `handle_token` (`s`) | `token` |
| `accept_label` (`s`) | `accept_label` |
| `modal` (`b`) | ignored: not modal in this version |
| `multiple` (`b`) | `multiple` |
| `directory` (`b`) | `directory` |
| `filters` (`a(sa(us))`) | `filters`: kind 0 to `patterns`, kind 1 to `mime_types` |
| `current_filter` (`(sa(us))`) | `current_filter`, mapped the same way |
| `choices` (`a(ssa(ss)s)`) | `choices`: `(id, label, [(id, label)], default)` |
| `current_name` (`s`) | `current_name` |
| `current_folder` (`ay`, NUL-terminated) | `current_folder` as a percent-encoded `file://` URI |
| `current_file` (`ay`, NUL-terminated) | `current_file` as a percent-encoded `file://` URI |
| `files` (`aay`, `SaveFiles`) | `files`; a name that is not UTF-8 makes the call fail with `InvalidArgument` |

Other options are ignored. A known option with the wrong type is refused at
the call with `org.freedesktop.portal.Error.InvalidArgument`, and no Request
is created.

The outcome becomes `Response(u response, a{sv} results)`, addressed to the
Request's owner alone:

| Outcome | `Response` |
|---|---|
| 0 | 0, with `uris` (`as`), `choices` (`a(ss)`) when the call had choices, `current_filter` (`(sa(us))`) when it had filters |
| 1 | 1, `{}` |
| 2, or `files.choose` refused (`BUSY`, `UNAVAILABLE`, any error), or `files` not registered, or lost | 2, `{}` |

The Request lifecycle:

- `Request.Close()` from the owner calls `files.choose.cancel`, removes the
  Request and sends no `Response`. `Close` from anyone else is refused with
  `org.freedesktop.portal.Error.NotAllowed`.
- When the owner leaves the session bus, portald cancels its requests.
- When portald loses the Bus or `files` unregisters, every open Request ends
  with `Response` 2. A Request is never left without an end: an app waiting on
  it must be able to tell failure from cancel.
- Only one end wins. A `Response` is sent at most once, and never after
  `Close`.
- portald subscribes to `files.chosen` before its first `files.choose`, and
  reads `files.choose.result` for each open request after it reconnects to the
  Bus.

## Acceptance

1. Over the Bus alone, under a nested compd: open, save (with replace
   confirmation) and save_files, each accepted, cancelled by the person,
   cancelled by the caller, and with the caller's connection dropped. Filters
   and choices round-trip. A path that is not UTF-8 round-trips as a URI.
2. Over D-Bus, with a persistent zbus caller fixture (a one-shot `gdbus call`
   disconnects before the `Response`): each method, `Request.Close` (no
   `Response`), the owner disconnecting, `files` absent (`Response` 2) and
   `Close` from another sender (refused).
3. A craft app built with rfd 0.17, on VT3: File > Open and File > Save As show
   the chooser; the file is opened and saved; Cancel leaves the app as it was.

## Open questions

1. FileChooser version: check the version and options against the current
   upstream interface XML before portald serves it.
2. Drive command names when one process has several chooser windows: a
   `window` argument on `files.ui.*`, or one Bus service per window.
3. Whether MixOS's own apps (Email's Attach files) move from the in-process
   `toolkit::file_chooser` to `files.choose`, keeping the toolkit chooser
   only for when the Bus or `files` is absent.
