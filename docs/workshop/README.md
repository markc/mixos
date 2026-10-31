---
title: Workshop
description: Workshop is the MixOS desktop app that lists the services and commands on this node's Bus, shows each command's description and the mesh peers, and calls one command once with a JSON body.
---

# Workshop

`workshop` (app id `dev.mixos.workshop`) is a native window onto the Bus
of the node it runs on. It asks noded which services are registered, asks
each service to describe its commands, and shows them as a tree. Select a command
to read its arguments and description, type a JSON body, and call it. The
reply is shown with its return code. Workshop also lists the mesh nodes
this node can see.

It is for people and agents who want to see what a node offers and try a
command by hand. It ships in the `mixos-desktop` package set. Nothing else in
the workspace depends on it.

Workshop is also the reference app for driving a window over the Bus.
Everything in its window is a Bus command, and
[Driving apps over the Bus](../docs/drive.md#workshop-the-worked-example)
uses it as the worked example.

The source is
[`apps/workshop`](https://github.com/markc/mixos/tree/main/apps/workshop).

## How it is built

### Saved arrangement

Workshop uses toolkit's shared saved-window store at
`<AppDirs::state()>/window.conf.mix`. With the ordinary XDG defaults this is
`$XDG_STATE_HOME/mixos/apps/workshop/state/window.conf.mix`, or
`~/.local/state/mixos/apps/workshop/state/window.conf.mix` without that
variable. AppDirs overrides are honoured.

It saves the selected command and row key, expanded tree rows, services
search, macro-window visibility, Traffic side, open and paused state and
filter, the services and Traffic panel shares, services scroll and the
Details outer scroll keyed by selection. The five window theme
choices remain independent; following the session stays following it.
Inner size and maximised state are restored on every desktop. On compd,
output, position and workspace are also remembered, without changing the
user's current workspace. Plasma and other Wayland desktops do not save or
restore position, output or workspace.

Selection and expansion return as live rows appear; restoring a selection
does not call or run anything. Keys absent after discovery are dropped.
Keeping the selected row visible takes priority over an old scroll offset.
Request bodies, dialogs, keyboard focus, replies and Traffic rows are not
saved. Reply and Traffic views start empty. Headless runs and tests use
memory only unless a test explicitly injects its own directory.

Writes are atomic, changed-only and coalesced, with a final flush at exit.
Malformed files or future store schemas load defaults and are preserved
untouched. `workshop.info` includes the toolkit's saved-state description.

Workshop follows the engine-first rule in
[AGENTS.md](https://github.com/markc/mixos/blob/main/AGENTS.md) §4.1. It is
two crates:

- **`inspector`** (`crates/inspector`): the headless engine. No UI and no
  I/O. It holds the protocol model, the discovery and call rules, and the
  handling of every `workshop.*` command.
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
| `model` | `Command`, `Snapshot` (services, each with its commands or an error; peers), `Selection`, the parsers for `noded.list`, `noded.peers`, `HELP` and `app.describe`, the limits, and `describe()`, the app's own command list |
| `engine` | `Engine`, `Effect`, `Dialog`, `Row` and `RowKind` (the services tree), `UiState` (what a person arranges), `Session` (the session theme) |
| `mesh` | bounded per-link and local traffic tally, event windows, reply correlation, topology, pings and injectable clock |
| `bus` | the connection, through the shared [citizen](../citizen/README.md) library: `start`, `probe`, `forward`, `show`, and `discover` |

Discovery reads `noded.info`, `noded.list` and `noded.peers`. The local node
appears first, followed by sorted peers. Each peer is queried through
`noded.<node>.bus`, then its services through `<service>.<node>.bus`.
Local membership and service identities are published as soon as they arrive,
without waiting for remote nodes or command descriptions. Discovery queries up
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
| `src/mesh.rs` | the native Mesh map, accessible nodes and links, and bounded dot animation state |
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

Workshop registers as `workshop` on the local broker. Its commands are
registered in
[`commands.conf.mix`](https://github.com/markc/mixos/blob/main/docs/docs/spec/bus/commands.conf.mix)
with owner `workshop`.

- Reading: `workshop.ping`, `workshop.info`, `workshop.tree`,
  `workshop.reply`, `workshop.actions`, `workshop.mesh`, plus `HELP` and `app.describe`.
- Changing state: `workshop.filter`, `workshop.expand`,
  `workshop.select_row`, `workshop.select`, `workshop.body`,
  `workshop.split`, `workshop.dock`, `workshop.dialog`, `workshop.theme`,
  `workshop.call`, `workshop.execute`, `workshop.refresh`,
  `workshop.show`, `workshop.quit`, `workshop.mesh.ping`.
- The window's drive commands, from the toolkit: `workshop.ui.tree`,
  `workshop.ui.click`, `workshop.ui.pointer`, `workshop.ui.scroll`,
  `workshop.ui.key`, `workshop.ui.type`, `workshop.ui.menu`,
  `workshop.ui.menu.list`, `workshop.ui.menu.open`,
  `workshop.ui.menu.close`, `workshop.ui.capture`, `workshop.window`
  and `workshop.window.state`.

Arguments, answers and refusal codes (`BUSY`, `DISCONNECTED`, `ARGUMENT`,
`UNKNOWN_COMMAND`, `UNKNOWN_ACTION`, `DISABLED`) are tabled in
[Driving apps over the Bus](../docs/drive.md#workshop-the-worked-example).
Tree rows use `node:<node>`, `service:<node>:<service>` and
`command:<node>:<service>:<command>`, for example `node:alpha`,
`service:alpha:noded` and `command:alpha:noded:noded.list`. A command selection
keeps its routed service address so calls reach the selected node.

Calling a command from Mix through Workshop:

```mix
$body = json_encode({service: "noded", command: "noded.list"})
send workshop workshop.call body=$body timeout=30
print $result
```

The answer is `{service, command, request_body, rc, body}`.

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
| `bus.call` | Bus › Call command | Ctrl+Enter |
| `mesh.ping` | Bus › Ping peers | |
| `help.shortcuts` | Help › Keyboard shortcuts | Ctrl+/ |
| `help.about` | Help › About Workshop | |
| `view.search` | title bar search button | Ctrl+F |
| `view.mode` | title bar light/dark button | |
| `view.traffic` | View › Traffic | |
| `view.traffic.auto` | View › Traffic placement › Auto | |
| `view.traffic.bottom` | View › Traffic placement › Bottom | |
| `view.traffic.right` | View › Traffic placement › Right | |

View › Theme adds the `view.theme.*`, `view.mode.*`, `view.style.*`,
`view.decorations.*` and `view.captions.*` choices. They set the theme of
this window only; the session's theme file is never written. The Help menu
opens with a field that searches every command. The desktop profile can
rebind the shortcuts (`keymaps.workshop.<id>`), and `workshop.actions`
reports the ones in force.

## Mesh map

The inspected peer's node panel includes a five-minute messages/s sparkline:
outgoing is solid and shaded, incoming dashed. Hover shows both sample values
and their age. The panel retains the last hovered or focused node so the
pointer can reach its graph. The existing bounded tally buckets supply exactly
300 one-second samples, oldest first through the current partial second,
with zero-filled gaps. Histories are included in the typed summary cache and
rebuilt only when its revision or wall-clock second changes.

Select **Nodes → Mesh** (`nodes:mesh`) to draw this node in the centre and
its peers around an ellipse in Details. Selecting Mesh refreshes `noded.info`
and `noded.peers`, and starts the same observation stream as Traffic's
**Observe traffic** action if it is off. Reconnecting refreshes the nodes and
restarts observation only if it was enabled before disconnecting. Turning
Observe off shows **Observation stopped** and a **Start observation** button;
selecting Mesh again also starts it. A refused stream shows the broker's reason
in Details. **Observation subscribed** reports the subscription state; the
last-event age reports receipt time without claiming that a quiet stream is live.
Traffic naming a peer absent from the map triggers a topology refresh, with
at most one refresh in flight.

Each spoke has separate outgoing and incoming curves. Their widths follow
the last five minutes of messages. Dots show observed messages in their
original order and spacing; replies launch after the request's travel time.
Rejected and dropped messages use the theme's danger colour. Local messages
make an expanding ring. Animation requests repaints only while moving, with
one scheduled wake for a delayed dot. A visible Mesh view also repaints once
per second to update ages and window widths. It schedules no age repaint when
hidden or when another row is selected, and this wake never polls the Bus.
Typed tally summaries are cached by tally revision and wall-clock second;
geometry is reused until the map rectangle, peers, font unit or theme changes.

Hover or focus a node to read its counts since start, over one minute and
five minutes, top services and commands, and its latest ping and age. The
local node panel includes local service traffic and totals across mesh links.
Every map node is keyboard focusable and has a distinct accessible name, for
example `beta · mesh peer` or `alpha · this node`. Use
`workshop.ui.click {label:"beta · mesh peer"}` to click the map; tree nodes
keep their plain names, such as `beta`. Clicking a map node selects
its `node:<name>` tree row; clicking a spoke opens Traffic filtered to its peer.

**Ping peers** in Details or **Bus → Ping peers** (`mesh.ping`) pings each
known peer once, sequentially, with a two-second deadline per peer. Nothing
pings on a timer. A failed ping leaves a danger ring until inbound traffic
from that peer or a newer successful ping clears it. Ping age is measured
from each peer's completion, rather than the start of the whole round.

`workshop.mesh {}` reads the map as JSON: `this_node`, `peers`, `nodes`,
`links` (message and byte totals, `out_1m`, `in_1m`, `out_5m`, `in_5m`,
top services and commands), `local`, `probe.results` (including `age_ms`),
`observer` (observation state, error and dropped counters), `nodes_error`,
and `recent`. `workshop.mesh.ping {}` starts a round and immediately returns
`{pinging:["beta","gamma"]}`; read `workshop.mesh` for its results. Both
commands accept every caller, local or mesh. Overlapping ping rounds return
`BUSY`; no known peers returns `UNAVAILABLE`.

Only this node's broker is observed. Dashed links between peers represent
traffic that Workshop cannot see. Broker events with `peer` identify the link
directly. Older events fall back to mesh addresses and reply correlation IDs;
unidentifiable messages appear in the unnamed-node count. Workshop excludes
its own observe, discovery and ping traffic from the tally. Named peers outside
the bounded map appear in unplaced-peer rows with their message counts;
overflow beyond the bounded link tally joins the unnamed count. Future broker
timestamps are clamped to receipt time for buckets; a clock step back discards
buckets more than five seconds ahead. Pausing Traffic
does not pause the map, and its counters are independent of Traffic's ring.

`workshop.tree` identifies the map with key `nodes:mesh` and kind `mesh_map`.
The peers group retains key `mesh` and kind `peers`. Saved arrangements from
before the Nodes root open that root once; later deliberate collapses survive.

All retained state is bounded: 64 named traffic links plus an unknown link,
40 distinct count keys plus an overflow key, 2,000 pending correlations,
2,000 recent events over 15 seconds, and a five-minute second-bucket window
(301 slots including the lower boundary, matching the reference tally).
The node list and ping results are capped at 64 peers. Retained event labels
are limited to 256 characters; oversized correlation IDs are not matched.

## Agents

The Agents branch, beside Nodes, shows this node's
[agentd](../agentd/README.md) sessions: New session…, the live ones with one
badge each (starting, ready, running, or question while a permission is
open), then Ended (n). Selecting a session reads its transcript back from
agentd's log and follows it live: the prompts, the agent's text, one row
per tool call (expand it for the raw update), plans and stops. Details has
Cancel turn, Close or Resume, a picker for each setting the agent reports
(model, effort, mode), the open permission questions with the time left
before agentd answers no, and a composer (Ctrl+Enter sends).

Every action is also a Bus command: `workshop.agents`,
`workshop.agents.select`, `.new`, `.prompt`, `.cancel`, `.close`,
`.resume`, `.set` and `.answer`. An agent session's own mixos-mcp is
refused the acting ones (`USE_AGENT`, rc 17), and so is a window it
drives; it calls `agent.*` directly, where agentd's own checks run. The
permission buttons answer only for a person at the window: a click driven
over the Bus is refused (`HUMAN_ONLY`).

## Traffic dock

Traffic uses the right pane when Auto sees a window at least 1280 logical
pixels wide, and the bottom pane otherwise. View › Traffic placement selects
Auto, Bottom or Right; the dock header's panel button switches between Bottom
and Right. Drag the pane boundary to resize it. Placement, right width and
bottom height are saved in the window's `UiState`, with separate sizes for
each side. Width is kept within 20–50% of the window, height within 12–50%.
The services split is a share of the width left beside Traffic, within 10–80%.
`workshop.dock {side:"auto", size}` records the requested share on both axes
(each within its own limits), so the size survives the first layout and later
crossings of Auto's width threshold. Startup resolves Auto from the saved
window width before the first frame.

Click Traffic in its header (or View › Traffic) to collapse it to a bottom
header strip; click again to reopen it. Right-side rows show time, command,
return code and milliseconds on the first line, with sender, service and
direction beneath. Both layouts expand a row to show its retained JSON.

`workshop.dock {side:"auto"|"bottom"|"right", size?:number, open?:bool}`
sets placement, optionally the active side's share (0–1, clamped to the limits
above) and whether the dock is open. Invalid values return `ARGUMENT` without
changing state. In Auto, size applies to the side used by the current window
(Bottom in a headless instance). `workshop.info` reports `ui.dock_side`,
`ui.dock_effective`, `ui.dock_size`, `ui.dock_width` and `ui.dock_height`.

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
  cover discovery, selection, the body, calls, the Bus commands, refusals,
  tickets and quitting, with no UI.
- `tests/shell.rs` runs the real `Shell` on a logging Bus handle (the
  `inspector` `testing` feature) and checks every reply, call and the final
  quit in order.
- `tests/drive.rs` drives the window over its Bus commands against the real
  view: clicks, typing, the title bar controls, menus.
- `tests/view.rs` checks the dialogs, the body buttons and their tooltips,
  and the search field through kittest.
- `tests/snapshot.rs` takes offscreen snapshots in several themes into
  `tests/snapshots/`. Regenerate with `UPDATE_SNAPSHOTS=1` and look at the
  images before committing them.
- `crates/inspector/src/mesh.rs` ports the Mesh Watch tally oracle with
  public example names and addresses, plus bounds, expiry, pause and engine
  effect tests. `tests/mesh.rs` checks accessible map controls and node panels;
  `tests/drive.rs` also drives map nodes and links through `workshop.ui.click`.
  The map snapshots `mesh_studio_dark.png` and `mesh_pro_light.png` use a fixed
  clock and no animation.
- `tests/registry.rs` checks that the command registry lists exactly the
  commands Workshop describes, and that read-only commands are class `query`
  ([command classes](../docs/decisions/2026-10-11-verb-classes.md)).
- `crates/inspector/tests/native.rs` runs discovery, calls and
  single-instance registration against an isolated real broker. It is
  ignored by default and needs `WORKSHOP_TEST_URL`.
- `crates/inspector/tests/discovery.rs` uses a fake broker with a node that
  never replies. Local and healthy rows arrive before its timeout, discovery
  settles within a bounded time, and the next refresh retries the failed node.

## Macro Bus commands

The shared [macro Bus surface](/macros/#bus-commands) uses this process's
catalogue and runner. Subscribe to `<service>.macros.finished` for completion, where `service` is
the instance’s registered Bus name (for example `term-1234` for a second Term
instance). Commands keep the app’s canonical prefix, such as `workshop.macros.run`.

| Command | Arguments | Result |
|---|---|---|
| `workshop.macros.list` | `{}` | Catalogue and diagnostics |
| `workshop.macros.reload` | `{}` | Catalogue and diagnostics |
| `workshop.macros.run` | `{id}` | Acceptance and run number |
| `workshop.macros.status` | `{}` | Current or last run |
| `workshop.macros.output` | `{}` | Retained output and dropped flag |
| `workshop.macros.stop` | `{}` | Whether termination was requested |
| `workshop.macros.new` | `{app,title}` | Create a user macro, select it and open its document; returns `{app,id,path,text,tier,catalogue}` |
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
Each app serves the same management commands through its own macro Session.
Select a macro group (`macros:<app>` or `macros:global`) or an existing macro
and choose **Macros → New macro…**, or use the **New macro…** button in
Details. Enter a title and choose Create. Workshop refreshes the catalogue,
selects the new file and gives its editor the keyboard. The action is
available while Workshop is idle. `workshop.execute {id:"macro.new"}` opens
the same dialog; `workshop.macros.new {app:"ced",title:"Tidy imports"}`
creates and opens it directly. Running apps receive their own `macros.new`
request; stopped apps and global macros use the shared local library.
Macro requests are capped at 8 KiB, except `put`: its envelope is capped at
1544 KiB and its text at 256 KiB.

The collapsible Traffic dock combines broker observations and native
`<service>.action.executed` / `<service>.macros.finished` topics. It retains
at most 1,024 rows and 8 MiB, reports dropped rows and redacts sensitive
topic fields before retaining them. Broker observation defaults to redacted
bodies; `none` is also supported. A refused observer leaves the reason in
the dock and keeps topic deliveries available. Any caller may observe. Run
`mix --serve tools/workshop_observe.mix --name workshop-probe` to check
the advertised observe extension and open and close a probe stream.

Open Workshop in an app's macro menu launches or focuses Workshop; a launch
with `--app ced --id tidy-imports` selects that macro. The global folder in
Dopus is the fallback when Workshop is not installed. Headless instances
use the same management and traffic commands.

Every macro command accepts every caller, local or mesh. This includes
global writes, moves between apps, `workshop.execute` for `macro.*` and
`bus.call`, and `workshop.call` with an explicit or selected target.
Built-in files remain read-only; path containment, argument validation,
request limits and the runner’s single-flight limit still apply.

Logging: `--debug[=LEVEL]` and live Bus controls are described in
[live application logging](../docs/dev/debugging.md).
