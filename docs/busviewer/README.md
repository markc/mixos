---
title: BusViewer
description: BusViewer is the MixOS desktop app that lists the services and verbs on this node's Bus, shows each verb's description and the mesh peers, and calls one verb once with a JSON body.
---

# BusViewer

`busviewer` (app id `dev.mixos.busviewer`) is a native window onto the Bus
of the node it runs on. It asks noded which services are registered, asks
each service to describe its verbs, and shows them as a tree. Select a verb
to read its arguments and description, type a JSON body, and call it. The
reply is shown with its return code. BusViewer also lists the mesh nodes
this node can see.

It is for people and agents who want to see what a node offers and try a
verb by hand. It ships in the `mixos-desktop` package set. Nothing else in
the workspace depends on it.

BusViewer is also the reference app for driving a window over the Bus.
Everything in its window is a Bus verb, and
[Driving apps over the Bus](../docs/drive.md#busviewer-the-worked-example)
uses it as the worked example.

The source is
[`apps/busviewer`](https://github.com/markc/mixos/tree/main/apps/busviewer).

## How it is built

BusViewer follows the engine-first rule in
[AGENTS.md](https://github.com/markc/mixos/blob/main/AGENTS.md) §4.1. It is
two crates:

- **`inspector`** (`crates/inspector`): the headless engine. No UI and no
  I/O. It holds the protocol model, the discovery and call rules, and the
  handling of every `busviewer.*` verb.
- **`busviewer`**: the egui shell. It draws the engine's state, runs the
  engine's effects on a small tokio runtime, and owns the command registry
  that the menus, the shortcuts and the Bus all go through.

The engine never does work itself. The shell feeds it events (Bus
deliveries, finished discoveries and calls, edits made in the window), and
the engine returns `Effect`s for the shell to carry out: `Discover`,
`Call`, `Show`, `Reply`, `Commands`, `Execute`, `Describe`, `Drive`, `Copy`
and `Exit`. Every effect that leaves the process carries a ticket. A
completion whose ticket is not the one in flight is ignored, so a late
reply can never land on a newer operation.

### inspector

| Module | What it holds |
|---|---|
| `model` | `Verb`, `Snapshot` (services, each with its verbs or an error; peers), `Selection`, the parsers for `noded.list`, `noded.peers`, `HELP` and `app.describe`, the limits, and `describe()`, the app's own verb list |
| `engine` | `Engine`, `Effect`, `Dialog`, `Row` and `RowKind` (the services tree), `UiState` (what a person arranges), `Session` (the session theme) |
| `bus` | the connection, through the shared [citizen](../citizen/README.md) library: `start`, `probe`, `forward`, `show`, and `discover` |

Discovery calls `noded.list` and `noded.peers`, then asks every service for
`HELP`, falling back to `app.describe`. It describes eight services at a
time. A service that fails to describe itself shows as an error row and
does not stop the others.

The limits are fixed in `model`:

- a JSON body is at most 65,536 bytes;
- a reply is shown up to 1,000,000 bytes, then marked as truncated.

### busviewer

| Module | What it holds |
|---|---|
| `src/main.rs` | options, single-instance launch, the eframe window |
| `src/lib.rs` | the string catalogue: `strings()`, `label()`, `label_with()` |
| `src/shell.rs` | `Shell`: applies events, settles effects, runs Bus work, follows the desktop profile, wires the toolkit drive layer |
| `src/view.rs` | `view()`: draws the engine's state and returns what the person did as `UiEvent`s; it never changes state itself |
| `src/commands.rs` | `registry()`: every command, with its menu, shortcut, icon and when it is enabled |
| `i18n/en/busviewer.ftl` | the English strings |

The shell applies one event and settles its effects before it reads the
next, so a later delivery can never change what an earlier command acts
on. Within one frame, edits are applied before commands, so a call uses
the body the person sees. Bus work wakes the window only when something
finished; nothing polls.

## Rules

- **One operation at a time.** A refresh or a call in flight makes the
  window busy. Bus requests that need it idle are refused with `BUSY`.
- **A call is sent once.** BusViewer never retries. A transport failure is
  reported as return code 10 with the outcome unknown.
- **No call on bad input.** An unadvertised target, a body over the limit
  or invalid JSON is refused before anything is sent. An empty body is
  allowed.
- **Closing answers everything.** Quit waits until accepted work is
  finished, and every Bus request already accepted gets an answer before
  the connection stops.
- **One window per session.** A second launch finds the running BusViewer
  on the Bus, asks it to show its window through compd, and exits.

## Bus

BusViewer registers as `busviewer` on the local broker. Its verbs are
registered in
[`verbs.conf.mix`](https://github.com/markc/mixos/blob/main/docs/docs/spec/bus/verbs.conf.mix)
with owner `busviewer`.

- Reading: `busviewer.ping`, `busviewer.info`, `busviewer.tree`,
  `busviewer.reply`, `busviewer.commands`, plus `HELP` and `app.describe`.
- Changing state: `busviewer.filter`, `busviewer.expand`,
  `busviewer.select_row`, `busviewer.select`, `busviewer.body`,
  `busviewer.split`, `busviewer.dialog`, `busviewer.theme`,
  `busviewer.call`, `busviewer.execute`, `busviewer.refresh`,
  `busviewer.show`, `busviewer.quit`.
- The window's drive verbs, from the toolkit: `busviewer.ui.tree`,
  `busviewer.ui.click`, `busviewer.ui.pointer`, `busviewer.ui.scroll`,
  `busviewer.ui.key`, `busviewer.ui.type`, `busviewer.ui.menu`,
  `busviewer.ui.menu.list`, `busviewer.ui.menu.open`,
  `busviewer.ui.menu.close`, `busviewer.ui.capture`, `busviewer.window`
  and `busviewer.window.state`.

Arguments, answers and refusal codes (`BUSY`, `DISCONNECTED`, `ARGUMENT`,
`UNKNOWN_VERB`, `UNKNOWN_COMMAND`, `DISABLED`) are tabled in
[Driving apps over the Bus](../docs/drive.md#busviewer-the-worked-example).
Tree rows are named by key, for example `service:noded` or
`verb:noded:noded.list`.

Calling a verb from Mix through BusViewer:

```mix
$body = json_encode({service: "noded", verb: "noded.list"})
send busviewer busviewer.call body=$body timeout=30
print $result
```

The answer is `{service, verb, request_body, rc, body}`.

## Commands and keys

Every menu item, shortcut and `busviewer.execute` id is one command in
`src/commands.rs`:

| Command | Menu | Key |
|---|---|---|
| `file.refresh` | File › Refresh services | Ctrl+R |
| `file.quit` | File › Quit | Ctrl+Q |
| `edit.format` | Edit › Format JSON body | Ctrl+Shift+F |
| `edit.clear` | Edit › Clear body | |
| `edit.copy` | Edit › Copy reply | |
| `bus.call` | Bus › Call verb | Ctrl+Enter |
| `help.shortcuts` | Help › Keyboard shortcuts | Ctrl+/ |
| `help.about` | Help › About BusViewer | |
| `view.search` | title bar search button | Ctrl+F |
| `view.mode` | title bar light/dark button | |

View › Theme adds the `view.theme.*`, `view.mode.*`, `view.style.*`,
`view.decorations.*` and `view.captions.*` choices. They set the theme of
this window only; the session's theme file is never written. The Help menu
opens with a field that searches every command. The desktop profile can
rebind the shortcuts (`keymaps.busviewer.<id>`), and `busviewer.commands`
reports the ones in force.

## Running it

```text
busviewer [--noded-url URL] [--service NAME] [--comp NAME]
```

- `--noded-url`: the broker to use. The default is the session's noded.
- `--service`: the Bus name to register as, `busviewer` by default.
- `--comp`: the compositor's service name, used to raise the window. The
  default is `$MIXOS_COMP_SERVICE`, or `comp`.

Service names may contain only ASCII letters, digits, `-`, `_` and `.`.

The window follows the desktop profile's look and keymaps from
[settingsd](../settingsd/README.md), through the
[settings](../settings/README.md) follower, and updates when they change.
With no theme chosen it starts in Studio Dark.

## Testing

On an authorised build worker at a pushed commit:

```text
cargo test --locked -p inspector -p busviewer
```

- The engine's unit tests (`crates/inspector/src/engine.rs`, `model.rs`)
  cover discovery, selection, the body, calls, the Bus verbs, refusals,
  tickets and quitting, with no UI.
- `tests/shell.rs` runs the real `Shell` on a logging Bus handle (the
  `inspector` `testing` feature) and checks every reply, call and the final
  quit in order.
- `tests/drive.rs` drives the window over its Bus verbs against the real
  view: clicks, typing, the title bar controls, menus.
- `tests/view.rs` checks the dialogs, the body buttons and their tooltips,
  and the search field through kittest.
- `tests/snapshot.rs` takes offscreen snapshots in several themes into
  `tests/snapshots/`. Regenerate with `UPDATE_SNAPSHOTS=1` and look at the
  images before committing them.
- `tests/registry.rs` checks that the verb registry lists exactly the
  verbs BusViewer describes, and that read-only verbs are class `query`
  ([verb classes](../docs/decisions/2026-10-11-verb-classes.md)).
- `crates/inspector/tests/native.rs` runs discovery, calls and
  single-instance registration against an isolated real broker. It is
  ignored by default and needs `BUSVIEWER_TEST_URL`.

## Macro Bus verbs

The shared [macro Bus surface](/macros/#bus-verbs) uses this process's
catalogue and runner. Subscribe to `<service>.macros.finished` for completion, where `service` is
the instance’s registered Bus name (for example `term-1234` for a second Term
instance). Verbs keep the app’s canonical prefix, such as `busviewer.macros.run`.

| Verb | Arguments | Result |
|---|---|---|
| `busviewer.macros.list` | `{}` | Catalogue and diagnostics |
| `busviewer.macros.reload` | `{}` | Catalogue and diagnostics |
| `busviewer.macros.run` | `{id}` | Acceptance and run number |
| `busviewer.macros.status` | `{}` | Current or last run |
| `busviewer.macros.output` | `{}` | Retained output and dropped flag |
| `busviewer.macros.stop` | `{}` | Whether termination was requested |
| `busviewer.macros.new` | `{title}` | Create a user per-app macro; `{id,title,path}`, without opening an editor |
