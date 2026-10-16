---
title: Decision — monitord and Monitor
description: One unprivileged service per host, monitord, samples the system and its processes, keeps history in SQLite and answers on the Bus, so any node's load can be read from anywhere on the mesh. Monitor is the egui task manager that draws it. Both are MIT OR Apache-2.0, built on permissive crates; GPL monitors are reference only.
---

# Decision — monitord and Monitor

Status: accepted 2026-10-16.

## The decision

- **`monitord`** (`services/monitord`, kind service, layer core) runs once
  per host as its own unprivileged user. It samples the system and its
  processes, keeps recent history in memory and longer history in SQLite,
  and answers read verbs on the Bus. It runs headless; a mesh node with no
  desktop is a first-class host.
- **`Monitor`** (`apps/monitor`, binary `monitor`, app id
  `dev.mixos.monitor`) is the egui task manager: processes, performance
  graphs, apps, services and users, in the shape people know from Windows
  Task Manager and GNOME Mission Center. It reads everything through
  monitord's verbs, never from `/proc` itself, so the same window shows any
  node on the mesh (`monitord.<node>.bus`).
- **monitord observes; it does not act.** No verb signals, renices or
  stops anything.
  - The `monitord` command line acts locally as the person who runs it:
    `monitord signal` and `monitord renice` make the system call directly
    and the kernel decides, exactly like `kill` and `renice`.
  - Acting across users or across the mesh is hostd's job, as closed verbs
    behind grants ([hostd decision](2026-10-12-hostd.md)), added one
    reviewed verb at a time.
- **A personality of the system binary.** monitord's library exports
  `main`; `cli/system` carries it
  ([one system binary](2026-10-12-one-system-binary.md)). The unit runs
  `/opt/mixos/bin/monitord serve`.

### History

| Tier | Resolution | Kept | Where |
|---|---|---|---|
| live | the sample interval (1 s) | 10 minutes | memory ring |
| short | 1 minute (avg, min, max) | 7 days | SQLite |
| long | 1 hour (avg, min, max) | 1 year | SQLite |

- SQLite (`rusqlite`, already in the system binary through the shell)
  holds only system-wide series: CPU, memory, swap, load, pressure, and one
  series per network interface and disk. Per-core and per-process series
  live in memory only; they are too many to keep.
- The database is `/var/lib/monitord/history.db`, owned by monitord. It is
  a cache: deleting it loses history, nothing else.
- Retention is configuration, so a small node can keep less.

### Sampling

- System-wide sampling runs all the time at 1 s. It reads a handful of
  `/proc` files and costs next to nothing.
- The process table is sampled only while someone is reading it: a
  `monitord.processes` call holds the process sampler on for 30 s. An idle
  mesh node does not walk `/proc` every second. A cold call samples twice,
  a short interval apart, so CPU percentages are never zero by default.
- monitord reads what an unprivileged user can read. A process's I/O
  counters and some of its details are readable only for monitord's own
  processes; elsewhere those fields are absent, never guessed. Wider reads
  are a later, reviewed capability (`CAP_SYS_PTRACE` or a hostd verb).

### On the Bus

- Read verbs (all queries): `monitord.status`, `monitord.snapshot`,
  `monitord.processes`, `monitord.process`, `monitord.series` and
  `monitord.history`. The verbs and their bodies are documented in the
  [monitord manual](../../monitord/README.md).
- A retained topic, `monitord.summary`, carries a small system summary
  every 5 s, so a dashboard of the whole mesh subscribes instead of
  polling.
- The reply bodies are a public contract (`contract = "public"`), versioned
  in monitord's changelog. Monitor depends on the JSON, not on monitord's
  Rust types. If a second component needs those types, they are promoted to
  a library by review (AGENTS.md §2.2).

### What it is built from

- **Phase 1 reads the kernel directly.** `/proc` and `/sys` parsers are
  small, and each one reads from a root directory, so tests run against a
  fixture tree, the way hostd's do. No collection crate is needed yet.
- **Later phases use permissive crates,** each checked against `deny.toml`
  when it enters:

  | Area | Crate |
  |---|---|
  | NVIDIA | `nvml-wrapper` (MIT OR Apache-2.0) |
  | AMD | `libamdgpu_top` (MIT) |
  | Intel, and GPU use per process on every vendor | `qmlib` (Apache-2.0) |
  | systemd units, login sessions | `zbus_systemd` (MIT OR Apache-2.0) |
  | battery | `starship-battery` (ISC) |
  | graphs, tables (Monitor) | `egui_plot`, `egui_table` (MIT OR Apache-2.0) |

- **Code may be lifted** from bottom (MIT) and systemd-manager-tui (MIT),
  with its notice kept (AGENTS.md §4).
- **GPL and other copyleft monitors are reference only:** Mission Center
  and magpie, Resources, nvtop. Nobody copies or translates their code.
  Where one of them shows *what* to read, the reader is written fresh from
  the kernel's documentation.

### Phases

1. monitord: CPU, memory, swap, pressure, load, disks, network interfaces,
   the process table and tree; history; the Bus verbs and topic; the command
   line, with local `signal` and `renice`.
2. Monitor: the egui window over phase 1, any node on the mesh.
3. GPUs: NVIDIA, AMD and Intel, with use per process.
4. systemd services and the Apps view (processes grouped by application).
5. Sensors, fans, battery, power.
6. Privileged readings (per-process network bytes over eBPF, SMART, RAPL
   energy, DMI) through a reviewed capability or hostd verbs.

## Why

- Every node on the mesh should be able to say what it is doing, in one
  shape, to any agent or window that asks, without ssh.
- The split between a headless sampler and a window is already the right
  one in Mission Center (magpie and its GTK frontend), but that code is
  GPL-3.0 and MixOS is MIT OR Apache-2.0. Permissive Rust crates cover
  most of the same ground, and what they don't cover is small.
- Keeping monitord read-only keeps it unprivileged, so it can run
  everywhere. Killing a process is already a kernel-checked system call for
  the local user; across users or nodes it is a privileged change, and
  those go through hostd.

## Consequences

- noded's built-in `mon` module (`mon.status`, `mon.processes`,
  `mon.props.*`) is superseded. Its verbs were never registered and nothing
  in the tree calls them. It is removed once monitord is installed on the
  mesh.
- monitord adds a system user, `monitord`, and the state directory
  `/var/lib/monitord`.
- Monitor's graphs and tables record their performance workloads (a
  2,000-process table, a 64-core grid) before they are optimised
  (AGENTS.md §4.1).

## Amendments

### 2026-10-16: Monitor's End task and Kill, and its widgets

- **Monitor acts on this node's processes the way the command line does.**
  End task (SIGTERM) and Kill (SIGKILL) are local system calls made as the
  person running Monitor, and the kernel decides. They are off while
  another node is shown, and no Bus verb of monitord's is involved. Acting
  on another node, or as another user, stays hostd's.
- **Phase 2 adds no graph or table crate.** The graphs are painted
  directly: a 64-core grid is 64 graphs a frame, which a plotting widget
  per graph would make heavy. The process table is `egui_extras`'
  `TableBuilder`, already in the tree. `egui_plot` and `egui_table` stay
  candidates for when a view needs what they add (zoom, axes, ragged rows).
