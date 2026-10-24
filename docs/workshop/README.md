---
title: Workshop
description: Workshop is the MixOS desktop app that lists the services and verbs on this node's Bus, shows each verb's description and the mesh peers, and calls one verb once with a JSON body.
---

# Workshop

`workshop` (app id `dev.mixos.workshop`) is a native window onto the Bus
of the node it runs on. It asks noded which services are registered, asks
each service to describe its verbs, and shows them as a tree. Select a verb
to read its arguments and description, type a JSON body, and call it. The
reply is shown with its return code. Workshop also lists the mesh nodes
this node can see.

It is for people and agents who want to see what a node offers and try a
verb by hand. It ships in the `mixos-desktop` package set. Nothing else in
the workspace depends on it.

Workshop is also the reference app for driving a window over the Bus.
Everything in its window is a Bus verb, and
[Driving apps over the Bus](../docs/drive.md#workshop-the-worked-example)
uses it as the worked example.

The source is
[`apps/workshop`](https://github.com/markc/mixos/tree/main/apps/workshop).

## How it is built

Workshop follows the engine-first rule in
[AGENTS.md](https://github.com/markc/mixos/blob/main/AGENTS.md) §4.1. It is
two crates:

- **`inspector`** (`crates/inspector`): the headless engine. No UI and no
  I/O. It holds the protocol model, the discovery and call rules, and the
  handling of every `workshop.*` verb.
- **`workshop`**: the egui shell. It draws the engine's state, runs the
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

Discovery reads `noded.info`, `noded.list` and `noded.peers`. The local node
appears first, followed by sorted peers. Each peer is queried through
`noded.<node>.bus`, then its services through `<service>.<node>.bus`.
Local membership and service identities are published as soon as they arrive,
without waiting for remote nodes or verb descriptions. Discovery queries up
to eight remote nodes concurrently, with a separate local slot. Each read has
a three-second deadline (`DISCOVERY_CALL_BUDGET`); info and list run together.
Across all nodes and refreshes, at most twelve discovery calls run at once:
six reserved for local reads and six shared by remote reads. A cancelled
description retains its permit until the native request finishes.
Discovery asks for `HELP`, falling back to `app.describe`, eight services
at a time per node, with a three-second budget for the description phase.
Every node update wakes the view. The status counts completed nodes and
unreachable nodes; unreachable rows include the reason and refresh retries them.
Failed discovery and descriptions appear beneath the
affected node or service. Each service retains its `ServiceInfo`; labels
show version and short SHA when advertised, and Details shows the full
identity including binary and dirty state.

The limits are fixed in `model`:

- a JSON body is at most 65,536 bytes;
- a reply is shown up to 1,000,000 bytes, then marked as truncated.

### workshop

| Module | What it holds |
|---|---|
| `src/main.rs` | options, single-instance launch, the eframe window |
| `src/lib.rs` | the string catalogue: `strings()`, `label()`, `label_with()` |
| `src/shell.rs` | `Shell`: applies events, settles effects, runs Bus work, follows the desktop profile, wires the toolkit drive layer |
| `src/view.rs` | `view()`: draws the engine's state and returns what the person did as `UiEvent`s; it never changes state itself |
| `src/commands.rs` | `registry()`: every command, with its menu, shortcut, icon and when it is enabled |
| `i18n/en/workshop.ftl` | the English strings |

The shell applies one event and settles its effects before it reads the
next, so a later delivery can never change what an earlier command acts
on. Within one frame, edits are applied before commands, so a call uses
the body the person sees. Bus work wakes the window only when something
finished; nothing polls.

## Rules

- **One operation at a time.** A refresh or a call in flight makes the
  window busy. Bus requests that need it idle are refused with `BUSY`.
- **A call is sent once.** Workshop never retries. A transport failure is
  reported as return code 10 with the outcome unknown.
- **No call on bad input.** An unadvertised target, a body over the limit
  or invalid JSON is refused before anything is sent. An empty body is
  allowed.
- **Closing answers everything.** Quit waits until accepted work is
  finished, and every Bus request already accepted gets an answer before
  the connection stops.
- **One window per session.** A second launch finds the running Workshop
  on the Bus, asks it to show its window through compd, and exits.

## Bus

Workshop registers as `workshop` on the local broker. Its verbs are
registered in
[`verbs.conf.mix`](https://github.com/markc/mixos/blob/main/docs/docs/spec/bus/verbs.conf.mix)
with owner `workshop`.

- Reading: `workshop.ping`, `workshop.info`, `workshop.tree`,
  `workshop.reply`, `workshop.commands`, plus `HELP` and `app.describe`.
- Changing state: `workshop.filter`, `workshop.expand`,
  `workshop.select_row`, `workshop.select`, `workshop.body`,
  `workshop.split`, `workshop.dialog`, `workshop.theme`,
  `workshop.call`, `workshop.execute`, `workshop.refresh`,
  `workshop.show`, `workshop.quit`.
- The window's drive verbs, from the toolkit: `workshop.ui.tree`,
  `workshop.ui.click`, `workshop.ui.pointer`, `workshop.ui.scroll`,
  `workshop.ui.key`, `workshop.ui.type`, `workshop.ui.menu`,
  `workshop.ui.menu.list`, `workshop.ui.menu.open`,
  `workshop.ui.menu.close`, `workshop.ui.capture`, `workshop.window`
  and `workshop.window.state`.

Arguments, answers and refusal codes (`BUSY`, `DISCONNECTED`, `ARGUMENT`,
`UNKNOWN_VERB`, `UNKNOWN_COMMAND`, `DISABLED`) are tabled in
[Driving apps over the Bus](../docs/drive.md#workshop-the-worked-example).
Tree rows use `node:<node>`, `service:<node>:<service>` and
`verb:<node>:<service>:<verb>`, for example `node:alpha`,
`service:alpha:noded` and `verb:alpha:noded:noded.list`. A verb selection
keeps its routed service address so calls reach the selected node.

Calling a verb from Mix through Workshop:

```mix
$body = json_encode({service: "noded", verb: "noded.list"})
send workshop workshop.call body=$body timeout=30
print $result
```

The answer is `{service, verb, request_body, rc, body}`.

## Commands and keys

Every menu item, shortcut and `workshop.execute` id is one command in
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
| `help.about` | Help › About Workshop | |
| `view.search` | title bar search button | Ctrl+F |
| `view.mode` | title bar light/dark button | |

View › Theme adds the `view.theme.*`, `view.mode.*`, `view.style.*`,
`view.decorations.*` and `view.captions.*` choices. They set the theme of
this window only; the session's theme file is never written. The Help menu
opens with a field that searches every command. The desktop profile can
rebind the shortcuts (`keymaps.workshop.<id>`), and `workshop.commands`
reports the ones in force.

## Running it

```text
workshop [--headless] [--noded-url URL] [--service NAME] [--comp NAME]
```

- `--noded-url`: the broker to use. The default is the session's noded.
- `--headless`: run the same engine and Bus port without a native window or
  renderer. Repaint events wake the logic thread. Use a distinct `--service`
  name for a test instance; duplicate registrations are refused.
- `--service`: the Bus name to register as, `workshop` by default.
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
cargo test --locked -p inspector -p workshop
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
  verbs Workshop describes, and that read-only verbs are class `query`
  ([verb classes](../docs/decisions/2026-10-11-verb-classes.md)).
- `crates/inspector/tests/native.rs` runs discovery, calls and
  single-instance registration against an isolated real broker. It is
  ignored by default and needs `WORKSHOP_TEST_URL`.
- `crates/inspector/tests/discovery.rs` uses a fake broker with a node that
  never replies. Local and healthy rows arrive before its timeout, discovery
  settles within a bounded time, and the next refresh retries the failed node.

## Macro Bus verbs

The shared [macro Bus surface](/macros/#bus-verbs) uses this process's
catalogue and runner. Subscribe to `<service>.macros.finished` for completion, where `service` is
the instance’s registered Bus name (for example `term-1234` for a second Term
instance). Verbs keep the app’s canonical prefix, such as `workshop.macros.run`.

| Verb | Arguments | Result |
|---|---|---|
| `workshop.macros.list` | `{}` | Catalogue and diagnostics |
| `workshop.macros.reload` | `{}` | Catalogue and diagnostics |
| `workshop.macros.run` | `{id}` | Acceptance and run number |
| `workshop.macros.status` | `{}` | Current or last run |
| `workshop.macros.output` | `{}` | Retained output and dropped flag |
| `workshop.macros.stop` | `{}` | Whether termination was requested |
| `workshop.macros.new` | `{title}` | Create a global user macro; `{id,title,path,scanned}`, without opening an editor; scan failures add `warning` and preserve the file |
| `workshop.macros.get` | `{id, app?}` | Read `{id,tier,path,text}`, at most 256 KiB |
| `workshop.macros.put` | `{id,text,app?}` | Save a user macro atomically and refresh its owning Session |
| `workshop.macros.remove` | `{id,app?}` | Delete a user macro; built-ins refuse with `DENIED` |
| `workshop.macros.move` | `{id,tier,app?,destination?}` | Move between user tiers; refuse a destination clash |
| `workshop.macros.copy` | `{id,app?}` | Copy a built-in into its corresponding user tier |
| `workshop.macros.open` | `{app,id?}` | Select a macro in Details, or expand the app's branch |
| `workshop.traffic` | `{limit?,filter?}` | Read up to 1,024 retained rows and the dropped count |
| `workshop.observe` | `{on,filter?,body?}` | Start or stop observation; body defaults to `redacted` |

The global branch has the stable key `macros:global`. Global user files live
directly in the user macro root, for example
`/home/user/.config/mixos/macros/mesh-ping.mix`. Workshop owns their Session.
App branches prefer the running app's catalogue and use the Library only when
the app is stopped, with a “not running” label. Running an app macro while its
app is stopped refuses with `UNAVAILABLE`. `app` selects the source app;
omit it or use `global` for globals. Moving a global into an app requires
`tier: "user-app"` and `destination: "ced"`, for example.

Details uses the shared editor document. Save (Ctrl+S), Run (Ctrl+Enter),
Stop, Copy to user, Move, Delete with confirmation, Open in Ced and Show in
Dopus dispatch through Workshop's command registry. A `-- target:` header
offers only its allowed apps; `any` offers running apps. The chosen app is
passed as `MIXOS_MACRO_TARGET`. Without a header, Run remains target-free.
Each app serves the same management verbs through its own macro Session.
Macro requests are capped at 8 KiB, except `put`: its envelope is capped at
1544 KiB and its text at 256 KiB.

The collapsible Traffic dock combines broker observations and native
`<service>.command.executed` / `<service>.macros.finished` topics. It retains
at most 1,024 rows and 8 MiB, reports dropped rows and redacts sensitive
topic fields before retaining them. Broker observation defaults to redacted
bodies; `none` is also supported. A refused observer leaves the reason in
the dock and keeps topic deliveries available. The host must explicitly
admit `workshop*` in `observe.allowed_services`. Run
`mix --serve tools/workshop_observe.mix --name workshop-probe` to check
the advertised observe extension and open and close a probe stream.

Open Workshop in an app's macro menu launches or focuses Workshop; a launch
with `--app ced --id tidy-imports` selects that macro. The global folder in
Dopus is the fallback when Workshop is not installed. Headless instances
use the same management and traffic verbs.

Macro-file writes (`put`, `remove`, `move`, `copy`, `new`) require the owning
process’s registered local service or Workshop (`workshop`, `workshop-*`).
Global writes and moves into another app’s catalogue require Workshop.
Workshop’s own mutating macro verbs, including forwarding with `app`, accept
only Workshop callers, preventing arbitrary callers from using it as a write
proxy. Refusals use rc 10, `DENIED`, and name the verb and rule before any
file effects. Read/run/status/output/stop/reload remain open.

The broker stamps service identity and local/mesh origin; mesh writes are
refused. Local service registration is not executable authentication: a
client able to register an unused Workshop service name gains its authority.
This is a Bus access rule, not filesystem or same-user code isolation.

Workshop’s Bus `execute` for mutating `macro.*` actions and `bus.call`,
implicit-target `call`, calls forwarding macro mutations or these proxy
operations, and UI click/pointer/key/type injection require Workshop too.
These paths can otherwise turn an untrusted caller into the manager.
Explicit `call` requests for read/run verbs and UI inspection remain open.
