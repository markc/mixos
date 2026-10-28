# File chooser contract: `files.choose`

Status: draft, version 0.1.0. DOpus serves the files provider and chooser
window; portald serves FileChooser v4. Revised after reviews by Sol, Fable and GLM.

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

The unit restarts on failure, as `portald@.service` does, so a crash costs
the open requests but not the next dialog. The chooser process serves
`files.*` and the drive commands only, never `dopus.*`; DOpus's registry test
must check each mode's command set separately.

Each start of the provider is an **incarnation**, named by a random 64-bit
value in lower-case hex. Requests belong to the incarnation that accepted
them and are lost with it.

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
| `filters` | list of filter, at most 64 | none | `open` and `save` only. See [Filters](#filters). |
| `current_filter` | filter | first of `filters` | `open` and `save` only: the filter in effect at first. See [Filters](#filters). |
| `choices` | list of choice, at most 16 | none | See [Choices](#choices). |
| `current_folder` | location | last folder used, else home | The folder shown first. |
| `current_name` | name | none | `save` only: the suggested file name. |
| `current_file` | location | none | `save` only: an existing file to save over; its folder and name are shown. Takes precedence over `current_folder` and `current_name`. |
| `files` | list of name, 1 to 256, unique | none | `save_files` only, and required: the names of the files about to be saved into the chosen folder. |
| `parent_window` | string, at most 256 bytes | `""` | The window the chooser belongs to. See [Placement](#placement). |
| `correlation` | string, 1 to 256 bytes | none | Opaque to the provider; echoed in the reply, the outcome and `files.choose.list`. |
| `on_behalf_of` | string, 1 to 128 bytes | none | The app a forwarding service asks for. portald sends `<unique bus name> <process name>`, for example `:1.42 fakeapp`. Shown to the person and used for limits and the remembered folder; it is not an authority. |

A **location** is an absolute path written as a UTF-8 string, or a `file://`
URI (RFC 8089, no host or `localhost`) whose percent-encoding carries the
exact bytes of a path. The path, decoded, is at most 4,096 bytes with no NUL
byte. Nothing else is accepted.

A **name** is one path component: a UTF-8 string, or `{"bytes": "<percent-encoded>"}`
for a name that is not UTF-8. Decoded, it is 1 to 255 bytes, with no `/` and
no NUL, and is neither `.` nor `..`.

Unknown fields are ignored. A known field of the wrong type or out of range,
a field used in the wrong mode, `save_files` without `files`, duplicate names
in `files`, or a body over 256 KiB is refused with `ARGUMENT`.

Reply, rc 0:

```json
{"request": "9f2c41d07ab3e655-17", "topic": "files.chosen", "correlation": "/org/freedesktop/portal/desktop/request/1_42/rfd1"}
```

`request` is `<incarnation>-<sequence>`, made by the provider. It is never
reused: the sequence only grows within an incarnation, and a new incarnation
has a new prefix. `correlation` is present when the caller gave one.

Refusals open no window and keep nothing:

- `BUSY` when 8 choosers are already open on the desk; when 2 are open for
  this caller; or when this caller has already made 10 requests in the last
  10 seconds (the 11th is refused). The **caller** for both limits is the pair (broker connection,
  `on_behalf_of`) when `on_behalf_of` is given, else the broker connection
  alone, so the apps behind portald are limited one by one;
- `UNAVAILABLE` when the chooser window cannot be opened (no display, for
  example).

**Shape.** A person can take longer than the Bus client waits for a reply
(60 s), and a topic is published only by the service that owns it, so the
answer cannot be a held reply or a topic the caller names. The shape is the
one `shell.execute`, `shell.execute.result` and `shell.execute.cancel` already
use: an immediate reply, an owner-published event and a pullable result.

**Ordering.** A caller subscribes to `files.chosen` before it calls
`files.choose`, or reads `files.choose.result` instead; a caller on another
node uses the result query. The provider sends the reply before the window
accepts any input, but the reply and the topic are separate deliveries, so a
caller keeps for a short while any outcome naming a request it has not yet
been told about. Once the reply is in, outcomes are matched by `request`
alone; `correlation` only links a parked outcome to a reply, and is applied
only when the outcome's `request` equals the reply's.

### `files.choose.result` (query)

Body `{request}`. Answers with the request's state:

- `{state: "open"}`: the person has not answered;
- `{state: "done", outcome: {...}}`: the outcome, exactly as published;
- `ARGUMENT` with `{"reason": "unknown"}` for a request this incarnation never
  made (another incarnation's included), or `{"reason": "expired"}` for one
  whose outcome is no longer kept.

Outcomes are kept for 10 minutes after they are decided, at most 64 of them;
when a 65th is decided, the oldest is dropped first. Reading an outcome does
not remove it. This command is for a caller that missed the publication.

### `files.choose.cancel` (intent)

Body `{request}`. Closes an open chooser; its outcome is response 1 with
`reason: "caller"`. Idempotent: a request already done, expired, never made
by this incarnation, or made by an earlier one answers rc 0 and changes
nothing. A malformed `request` is `ARGUMENT`.

Any caller may cancel any request, as the open Bus requires
(`docs/docs/decisions/2026-10-26-open-bus-for-agents.md`); drive input can
press Cancel in the window anyway. It is also how choosers left by a caller
that has gone are cleared before noded's connection-closed notice exists.

### `files.choose.list` (query)

Body none. Answers
`{incarnation, requests: [{request, mode, title, caller, on_behalf_of, correlation, opened_ms}]}`
for every open chooser, oldest first. `caller` is `citizen::caller_key` of
the connection that asked.

## Outcome

When a request ends, one outcome is published once on topic `files.chosen`
and kept for `files.choose.result`:

```json
{
  "request": "9f2c41d07ab3e655-17",
  "correlation": "/org/freedesktop/portal/desktop/request/1_42/rfd1",
  "response": 0,
  "reason": "chosen",
  "uris": ["file:///home/user/Pictures/beach.png"],
  "current_filter": {"name": "Images", "patterns": ["*.png", "*.jpg"], "mime_types": []},
  "choices": {"encoding": "utf8", "reencode": "true"}
}
```

| `response` | `reason` | When |
|---|---|---|
| 0 | `chosen` | The person accepted. |
| 1 | `cancelled` | The person pressed Cancel or Escape, or closed the window. |
| 1 | `caller` | `files.choose.cancel`, or the caller's broker connection closed. |
| 2 | `failed` | The chooser failed after it opened. `message` says why. |

Each request has exactly one outcome; the first end wins and later ones are
dropped. Response codes are the portal's, so portald passes them on
unchanged.

- `uris` are `file://` URIs, percent-encoded from the exact bytes of each path.
  On `open`, there is one unless `multiple` is set, never none. On `save`,
  there is exactly one. On `save_files`, there is one per name in `files`, in
  the same order, each exactly `<chosen folder>/<name>`; the provider checks
  this before it accepts.
- `current_filter` is the filter in effect when the person accepted. It is
  present whenever a filter was in effect, from `filters` or from
  `current_filter` alone.
- `choices` maps every choice's `id` to its value: an option id, or `"true"`
  or `"false"` for a check box.
- `uris`, `current_filter` and `choices` are present only when `response` is 0.

The topic is published by `files` alone. Every subscriber sees every outcome;
the Bus is open (`docs/docs/decisions/2026-10-26-open-bus-for-agents.md`),
and an outcome carries paths, never file contents.

## Filters

A filter is `{name, patterns, mime_types}`: a label of 1 to 128 bytes, and
together 1 to 512 entries in two lists of strings of 1 to 256 bytes (an "All
images" filter can list hundreds):

- `patterns`: shell globs (`*`, `?`, `[...]`), matched against the file name,
  case sensitive;
- `mime_types`: MIME types or `type/*`, matched by the type the shared MIME
  database gives the file.

`current_filter` alone, with no `filters`, is the one filter in effect and the
filter menu is hidden. With `filters`, a `current_filter` equal to one of them
(same name and lists) selects it; one that is not equal to any is added at the
top of the menu and selected. Folders are always shown, so the person can
move between them. When `save` is used with a filter whose only pattern is
`*.ext`, a typed name without an extension gets that one.

## Choices

A choice is `{id, label, options, default}`:

- `id` and `label` are strings of 1 to 64 and 1 to 128 bytes; ids are unique;
- `options` is a list of up to 32 `{id, label}` with unique ids. When it is
  empty the choice is a check box;
- `default` is optional. For a list it is one of the option ids, and the
  first option when absent. For a check box it is `"true"` or `"false"`, and
  `"false"` when absent.

They are shown under the file list, in order.

## Behaviour

- **No timeout.** A chooser waits for the person as long as it takes.
- **Save.** When the chosen file exists, the person confirms replacing it
  before Accept succeeds. `save_files` does the same once for all the names
  that exist; unlike some portal backends it never renames to avoid a
  collision, so each URI is exactly `<folder>/<name>`. The chooser never
  creates, truncates or replaces anything; the caller does that with the URIs
  it receives.
- **Typed names.** A typed save name containing `/` is split at the last `/`.
  The folder part, relative to the shown folder or absolute, is navigated to
  when it exists; the tail stays in the name field as the name. An empty tail
  (`Documents/`) just navigates. A missing folder part refuses Accept in the
  window with a message.
- **Remembered folder.** The last folder accepted is kept per
  `on_behalf_of`, else per caller, and is where the next chooser for it opens
  when no `current_folder` or `current_file` is given. At most 128 callers are
  remembered, with the least recently used dropped first.
- **Who is asking.** When the caller is not `portald`, the window names the
  caller by `citizen::caller_key`, so a person can tell an agent's request
  from an app's. `on_behalf_of` is the caller's claim, not an identity: it is
  shown as a claim ("for fakeapp, says portald"), visibly apart from the
  caller's name, and never in its place.
- **Drive.** Each chooser window answers the toolkit drive commands
  (`docs/docs/drive.md`) as `files.ui.*` and `files.window*`, with the
  request id as the `viewport`. With more than one chooser open, a drive
  command without `viewport` is `ARGUMENT`. They are registered with the code.
- **Caller leaves.** When the caller's broker connection closes, its open
  choosers close with response 1, `reason: "caller"`. This needs a notice
  from noded that a connection closed, which does not exist yet (see
  [Losing a party](#losing-a-party)).
- **Provider stops.** If `files` stops, every open request is lost with its
  incarnation; a new incarnation knows nothing of them. Nothing is published
  for a lost request, so the topic is not a liveness guarantee: a caller that
  must not wait forever checks `files.choose.result` (`unknown` means lost)
  when `files` unregisters, or on a deadline of its own.

## Losing a party

Two notices are needed, and noded publishes neither today. Step 2 adds both
to noded as owner-published topics: a service registered or unregistered, and
a connection closed. Until they land, v1 does this, and the stopgaps are
removed when they do:

- **Provider lost, or an outcome missed (portald's view).** While at least
  one Request is not ended, portald reads `files.choose.list` every 5
  seconds. Two consecutive list failures or a changed `incarnation` mean
  `files` was lost. A successful list clears the failure count. On loss
  while the Bus is still live, portald also cancels each open chooser. An
  **open** Request whose `request` is missing from the list, with the
  incarnation unchanged, was decided and its publication missed: portald
  reads `files.choose.result` and sends the `Response` (`done`), or
  `Response` 2 (`unknown`). This is a bounded, temporary exception to the
  no-polling rule: it runs only while a person has a dialog open.
- **portald lost (the provider's view).** At start, portald reads
  `files.choose.list` and cancels every request whose `correlation` is a
  portal Request path (`/org/freedesktop/portal/desktop/request/…`): none of
  them can belong to the new portald.
- **Other callers lost.** Their choosers stay open until the person answers
  or anyone cancels them; the outcome is published and kept as usual.

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
for ScreenCast: `handle_token` is 1 to 64 of `A-Z a-z 0-9 _`, else the call
is refused with `InvalidArgument`; a token whose path already holds a live
Request from the same sender is refused the same way. Then portald calls
`files.choose` with the mapped body, `correlation` set to the Request's
object path, and `on_behalf_of` set to the caller's unique bus name and
process name, the process name cut to fit 128 bytes.

| Portal | `files.choose` |
|---|---|
| method `OpenFile` / `SaveFile` / `SaveFiles` | `mode` `open` / `save` / `save_files` |
| `parent_window` argument | `parent_window` |
| `title` argument | `title` (the mode's default when empty) |
| `handle_token` (`s`) | not sent: it names the Request path only |
| `accept_label` (`s`) | `accept_label` |
| `modal` (`b`) | ignored: not modal in this version |
| `multiple` (`b`) | `multiple` |
| `directory` (`b`) | `directory` |
| `filters` (`a(sa(us))`) | `filters`: kind 0 to `patterns`, kind 1 to `mime_types`; not sent for `SaveFiles` |
| `current_filter` (`(sa(us))`) | `current_filter`, mapped the same way; not sent for `SaveFiles` |
| `choices` (`a(ssa(ss)s)`) | `choices`: `(id, label, [(id, label)], default)`; an empty default is left out |
| `current_name` (`s`) | `current_name` |
| `current_folder` (`ay`) | `current_folder` as a percent-encoded `file://` URI |
| `current_file` (`ay`) | `current_file` as a percent-encoded `file://` URI |
| `files` (`aay`, `SaveFiles`) | `files`, each the last component of the entry (as other portal backends do); `{"bytes": ...}` when not UTF-8 |

Byte strings (`ay`) carry exactly one trailing NUL, which is removed; a byte
string without it, or with a NUL anywhere else, is refused. Other options are
ignored.

**Lenient at the D-Bus edge.** rfd and others show any portal error as a
cancel, so a refusal is a dialog that silently never appears. portald
therefore refuses only a known option with the wrong D-Bus type, a NUL
violation, or `SaveFiles` with no usable `files`, with
`org.freedesktop.portal.Error.InvalidArgument` at the call and no Request.
Everything else is made to fit: an option the method's mode does not take
(`multiple` or `directory` on a save, for example) is not sent; a location or
name that cannot be made valid is not sent; duplicate `files` names keep the
first; an empty string becomes the field's default;
a label over its limit is cut at a character boundary; a list over its limit
keeps its first entries; an invalid entry (a pattern over 256 bytes, a choice
with a duplicate id) is dropped. portald logs each adjustment, except a normal
empty `title` or `accept_label` that selects the default.

The outcome becomes `Response(u response, a{sv} results)`, addressed to the
Request's owner alone:

| Outcome | `Response` |
|---|---|
| 0 | 0, with `uris` (`as`), `choices` (`a(ss)`) when the call had choices, `current_filter` (`(sa(us))`) when a filter was in effect |
| 1 | 1, `{}` |
| 2, or `files.choose` refused (`BUSY`, `UNAVAILABLE`, any error), or `files` not registered, or lost | 2, `{}` |

### Request lifecycle

Each portal Request is in one of four states. Every transition happens under
one lock, and a Request leaves the table only when it is ended.

| State | Entered when | Close (owner) or owner leaves | Outcome for it arrives | `files.choose` answers | Bus or `files` lost |
|---|---|---|---|---|---|
| **sending** | the method returns the Request path, before `files.choose` is sent | to **closing** | parked by `correlation` until the reply | ok: to **open**, then apply a parked outcome whose `request` matches; refused: `Response` 2, ended; no reply in time: `Response` 2, to **closing** | `Response` 2, ended |
| **open** | `files.choose` answered with a request id | send `files.choose.cancel`; ended, no `Response` | matched by `request`: `Response` as mapped, ended | — | `Response` 2, ended |
| **closing** | Close came while sending, or the reply timed out | — | dropped | ok: send `files.choose.cancel`, ended; refused: ended | ended |
| **ended** | a `Response` was sent, or Close was handled | — | dropped | dropped | — |

- portald subscribes to `files.chosen` before it serves FileChooser, and
  again on every reconnect to the Bus.
- A Request in **closing** that never gets a reply is also swept: portald
  looks for its `correlation` in `files.choose.list`, cancels any match, and
  ends it.
- The sweep also checks each Request owner's D-Bus name, recovering a missed
  disconnect signal as Close (cancel, no Response). A failed or timed out cancel
  is retried by the five-second sweep, up to three attempts in total; rc 0
  acknowledges cancellation, including unknown or already ended requests.
- `Close` from anyone but the owner is refused with
  `org.freedesktop.portal.Error.NotAllowed`.
- A lost connection is final. When portald loses the Bus, or `files` is lost
  (see [Losing a party](#losing-a-party)), every Request not yet ended gets
  `Response` 2 and ends. After reconnecting, portald does not try to recover
  them; it cancels their choosers as described there.
- A Request is never left without an end, so an app waiting on it can tell
  failure from cancel. A `Response` is sent at most once, and never after
  `Close`.

## Acceptance

1. Over the Bus alone, under a nested compd: open, save (with replace
   confirmation) and save_files, each accepted, cancelled by the person,
   cancelled by the caller, and with the caller's connection dropped. Filters
   (including a standalone `current_filter`) and choices (including an
   absent default) round-trip. A path and a `save_files` name that are not
   UTF-8 round-trip. Names `..`, `a/b`, an empty name and duplicates are
   refused. Request ids are not reused across a provider restart. The limits
   and the outcome eviction refuse and evict as written.
2. Over D-Bus, with a persistent zbus caller fixture (a one-shot `gdbus call`
   disconnects before the `Response`): each method; two apps using the same
   `handle_token` at once; three apps with dialogs open at once (none `BUSY`);
   `Request.Close` while sending and while open (no `Response`, no chooser
   left open), then the same `handle_token` reused at once (the new Request
   gets only its own outcome); the owner disconnecting; a `files.choose`
   reply that times out (`Response` 2, and the late window is cancelled);
   `files` absent, and killed mid-request (`Response` 2 within 10 s); an
   empty `accept_label` and a 300-pattern filter (accepted, adjusted); and
   `Close` from another sender (refused).
3. A craft app built with rfd 0.17, on VT3: File > Open and File > Save As show
   the chooser; the file is opened and saved; Cancel leaves the app as it was.

## Settled questions

1. **FileChooser version 4.** Checked against upstream: every mapped option
   exists at version 4 (`directory` since 3); `SaveFiles` takes no `filters`
   or `current_filter`.
2. **Drive with several windows:** the request id is the drive `viewport`
   (see Behaviour).
3. **MixOS's own apps** (Email's Attach files) move to `files.choose` as a
   task of their own once acceptance 3 passes. `toolkit::file_chooser` stays
   only for when `files.choose` is `UNAVAILABLE` or `files` is absent.
4. **Connection-closed notice:** v1 does not wait for it (see
   [Losing a party](#losing-a-party)); noded gains the two topics in step 2.
