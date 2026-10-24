---
title: The system monitor service
description: How monitord samples a host's CPU, memory, disks, network and processes, keeps history in memory and SQLite, and answers read commands on the Bus, so any node's load can be read from anywhere on the mesh.
---

# The system monitor service

`monitord` runs once on every MixOS host, as its own unprivileged user. It
samples the system every second, keeps history, and answers questions about
the host on the Bus. It only reads. The reasons are in the
[monitord decision](../docs/decisions/2026-10-16-monitord-and-monitor.md).

Monitor, the task manager window, draws what monitord serves. Agents and
scripts ask it directly.

## Asking it

On the Bus, the service is `monitord`. Any node's monitord is reachable
over the mesh as `monitord.<node>.bus`:

```mix
send monitord monitord.snapshot
print($reply.cpu.busy)

send monitord.alpha.bus monitord.processes sort=memory limit=5
```

Every command is a query: it changes nothing. A refusal comes back with `rc`
10 and a body whose `status` says why: `bad_request`, `not_found`,
`unknown_command`, `failed` or `busy`.

| Command | Body | Returns |
|---|---|---|
| `monitord.status` | — | Version, uptime, sample interval, whether the process sampler is running, and the history database's size and retention |
| `monitord.snapshot` | — | The system now (below) |
| `monitord.processes` | `sort`, `reverse`, `limit`, `filter`, `user`, `tree`, `kernel_threads` | The process table |
| `monitord.process` | `pid` | One process in full |
| `monitord.series` | — | The history series held: `live` and `stored` |
| `monitord.history` | `series`, `from` or `since`, `to`, `max_points` | Points for each series |

Unknown body fields are refused, so a typo is an error, not a silent
default.

### `monitord.snapshot`

Rates and percentages are over the last sample interval (`interval_s`).
Sizes are bytes; rates are per second.

- `host`, `ts` (Unix seconds), `uptime_s`, `boot_time`.
- `cpu`: `model`, `cores`, and `busy`, `user`, `system`, `iowait`, `steal`
  in percent; `per_core`, a list of the same per CPU with `mhz`;
  `context_switches_ps`, `forks_ps`.
- `memory`: `total`, `available`, `used` (`total - available`), `free`,
  `cached`, `buffers`, `shared`, `dirty`. `swap`: `total`, `used`, `free`.
- `load`: `one`, `five`, `fifteen`, `runnable`, `tasks`.
- `pressure`: for `cpu`, `memory` and `io`, the share of time tasks
  stalled (`some`, and `full` where the kernel reports it), each as
  `avg10`, `avg60`, `avg300`. Absent on kernels without PSI.
- `tasks`: `total`, `running`, `blocked`.
- `disks`: each whole disk with `model`, `rotational`, `read_bps`,
  `write_bps`, `reads_ps`, `writes_ps` and `busy` (percent of the interval
  with I/O in flight). Loop and RAM devices are left out.
- `filesystems`: each real filesystem once per device, with `mount`,
  `device`, `kind`, `total`, `used`, `available`.
- `net`: each interface but loopback, with `state`, `physical`,
  `speed_mbps`, `rx_bps`, `tx_bps` and the running totals.

A value monitord can't know is `null`, never zero.

### `monitord.processes`

```text
{"sort":"cpu", "limit":100, "filter":"firefox", "user":"alice", "tree":false}
```

- `sort` is `cpu` (the default), `memory`, `io`, `pid`, `name` or `user`.
  Numbers sort largest first and names A to Z; `reverse` flips it.
- `limit` defaults to 100; 0 means every row.
- `filter` matches the name or command line, ignoring case.
- `tree` puts parents before children, siblings in sort order, each row
  with its `depth`. A filter keeps a match's ancestors, so the tree still
  joins up.
- The kernel's own threads are left out unless `kernel_threads` is true.

The reply has `total` (rows that matched), `count` (rows returned) and
`rows`. Each row has `pid`, `ppid`, `name`, `cmdline`, `user`, `uid`,
`state`, `cpu`, `memory` (resident), `virtual_memory`, `threads`, `nice`,
`priority`, `started_at`, `read_bps`, `write_bps`, `unit` (the systemd
service or scope) and `kernel_thread`.

`cpu` is a share of the whole machine, as Task Manager shows it: one busy
core of eight is 12.5.

The process table is sampled only while someone reads it. A call keeps the
sampler running for 30 seconds. The first call after a quiet spell takes
two scans 0.3 s apart, so it answers in about a third of a second.

monitord runs unprivileged, so another user's I/O counters, open files and
working directory are unreadable to it. Those fields are `null`.

### `monitord.process`

`{"pid": 1234}` returns the table's fields plus `exe`, `cwd`, the whole
`cmdline`, `cpu_time_s`, `memory_peak`, `swap`, `read_bytes`,
`write_bytes`, `open_files`, `voluntary_switches`,
`involuntary_switches`, `cgroup` and `children`.

### `monitord.history`

```text
{"series":["cpu.busy","net.*"], "since":3600, "max_points":600}
```

- `series` is one name or a list. A name ending in `*` matches a prefix:
  `cpu.core.*` is every core.
- Give `from` (Unix seconds) or `since` (seconds back from `to`); the
  default is the last ten minutes. `to` defaults to now.
- Each point is `[time, average, minimum, maximum]`. Points are merged into
  buckets of `step_s` seconds so a series has at most `max_points` (default
  600).
- `tier` says where the points came from:

| Tier | Resolution | Kept | Series |
|---|---|---|---|
| `live` | every sample | 10 minutes | all, per core included |
| `minute` | 1 minute | 7 days | system-wide |
| `hour` | 1 hour | 1 year | system-wide |

The series are `cpu.busy`, `cpu.user`, `cpu.system`, `cpu.iowait`,
`cpu.steal`, `mem.used`, `mem.available`, `mem.cached`, `swap.used`,
`load.1`, `load.5`, `load.15`, `psi.<cpu|memory|io>.<some|full>`,
`disk.<name>.<read|write|busy>` and `net.<name>.<rx|tx>`, plus, in memory
only, `cpu.core.<n>.busy` and `cpu.core.<n>.mhz`.

### The summary topic

Every 5 seconds monitord publishes a small retained summary on
`monitord.summary`: host, CPU busy and core count, memory, swap, load, total
network and disk rates, the 10-second pressure figures and the task count.
A mesh dashboard subscribes to it instead of polling every node.

## The command line

The same binary works with no daemon running. It reads the kernel itself,
so it answers on a node's console or over ssh either way:

```text
monitord snapshot            the system now, sampled over half a second
monitord ps --sort memory    the process table (--tree, --user, --filter, --all)
monitord history cpu.busy --since 86400    stored history from the database
monitord signal 1234 TERM    send a signal, as you
monitord renice 1234 10      change a nice value, as you
```

`--json` prints what the matching command returns. `signal` and `renice` run as
the person who typed them, and the kernel decides whether they may. No Bus
command signals or renices anything: acting across users or nodes is
[hostd](../hostd/README.md)'s job.

## Running it

`monitord.service` runs `/opt/mixos/bin/monitord serve` as the user
`monitord`, with no capabilities and a strict sandbox. The history
database is `/var/lib/monitord/history.db`; it is a cache, and deleting it
loses history and nothing else.

`serve` takes:

- `--interval-ms` (default 1000);
- `--db PATH` or `--no-db`;
- `--minute-days` (default 7) and `--hour-days` (default 365);
- `--no-bus`, to sample and record without joining the Bus.

When the broker is down, monitord keeps sampling and recording, and joins
when it comes back.

## Testing

```text
cargo test -p monitord
```

The kernel readers take the text of each file, and the command tests run
against a fixture `/proc` and `/sys` tree, so no test needs a real host or
a broker.
