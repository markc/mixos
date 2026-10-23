---
title: Monitor
description: Monitor is the MixOS task manager, showing the processes and performance that monitord reports for this node or any node on the mesh, with End task and Kill for this node's processes.
---

# Monitor

Monitor is the task manager. It shows what [monitord](../monitord/README.md)
reports, for this node or for any node on the mesh: the processes, and graphs
of CPU, memory, each disk and each network interface over the last minute.
It reads nothing from the system itself, so a node with no desktop can be
watched from one that has one. The reasons are in the
[monitord and Monitor decision](../docs/decisions/2026-10-16-monitord-and-monitor.md).

## The window

- **Processes:** every process, sorted by any column (click a header; click
  it again to reverse), filtered by name or command line, or shown as a tree
  with parents above their children. CPU is a share of the whole machine:
  one busy core of eight is 12.5.
- **Performance:** CPU, memory, each disk and each network interface down
  the side, each with a small graph. The chosen one fills the rest: a
  minute's graph and its figures. CPU adds a graph for every logical
  processor; a disk adds its read and write rates and its filesystems; an
  interface its receive and send rates.
- **Node:** the chooser in the tool bar lists this node and the mesh peers.
  Choosing one reads that node's monitord as `monitord.<node>.bus`. The
  graphs start full: Monitor asks monitord for the last minute first.

Monitor reads once a second. Pause (Ctrl+P) holds the readings still;
Refresh (Ctrl+R) reads once.

## End task and Kill

End task (Delete) asks the selected process to finish (SIGTERM). Kill
(Shift+Delete) stops it at once (SIGKILL). Both act only on this node's
processes, as the person running Monitor, and the kernel decides whether
they may: the same as `kill` in a terminal. They are off while another node
is shown. Acting on another node, or as another user, is
[hostd](../hostd/README.md)'s job.

## Agents

Everything in the window is a command or a verb, under `monitor`:

| Verb | Body | Does |
|---|---|---|
| `monitor.info` | — | The window's state: node, page, section, sort, filter, selection, and the latest readings |
| `monitor.rows` | — | The process rows in the order shown |
| `monitor.node` | `node` (a name, or null for this node) | Watch another node |
| `monitor.tab` | `tab` (`processes` or `performance`) | Show a page |
| `monitor.section` | `section` (`cpu`, `memory`, `disk:<name>`, `net:<name>`) | Show one performance section |
| `monitor.sort` | `column`, optional `reverse` | Sort the table |
| `monitor.filter` | `text` | Filter the table |
| `monitor.tree` | `on` | Tree or flat |
| `monitor.select` | `pid` (or null) | Select a row |
| `monitor.refresh` | — | Read the node now |
| `monitor.commands`, `monitor.execute` | `id` | List and run commands (`process.end`, `view.pause`, ...) |
| `monitor.dialog`, `monitor.theme`, `monitor.show`, `monitor.quit`, `monitor.ping` | | As every MixOS app |

The window's drive verbs (`monitor.ui.*`, `monitor.window*`) are the
toolkit's, described in [driving an app](../docs/drive.md).

```mix
send monitor monitor.node node=beta
send monitor monitor.sort column=memory
send monitor monitor.rows
```

## Running it

```text
monitor [--node NAME] [--noded-url URL] [--service NAME]
```

A second launch raises the first window. `--node` starts on a mesh node
instead of this one.

## Testing

```text
cargo test -p monitor -p vitals
```

The engine (`vitals`) is tested without a window or a Bus. The window is
checked by offscreen snapshots (`apps/monitor/tests/snapshots/`); run with
`UPDATE_SNAPSHOTS=1` to redraw them, and look at the images before
committing. The snapshot suite also times the representative workload, a
2,000-process table and a 64-core grid, and prints it.

## Macro Bus verbs

The shared [macro Bus surface](/macros/#bus-verbs) uses this process's
catalogue and runner. Subscribe to `<service>.macros.finished` for completion, where `service` is
the instance’s registered Bus name (for example `term-1234` for a second Term
instance). Verbs keep the app’s canonical prefix, such as `monitor.macros.run`.

| Verb | Arguments | Result |
|---|---|---|
| `monitor.macros.list` | `{}` | Catalogue and diagnostics |
| `monitor.macros.reload` | `{}` | Catalogue and diagnostics |
| `monitor.macros.run` | `{id}` | Acceptance and run number |
| `monitor.macros.status` | `{}` | Current or last run |
| `monitor.macros.output` | `{}` | Retained output and dropped flag |
| `monitor.macros.stop` | `{}` | Whether termination was requested |
| `monitor.macros.new` | `{title}` | Create a user per-app macro; `{id,title,path}`, without opening an editor |
