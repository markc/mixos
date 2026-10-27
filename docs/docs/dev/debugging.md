# Live application logging

Every MixOS app and compd accepts `--debug[=LEVEL]`. `--debug` means
`--debug=debug`; the levels are `off`, `error`, `warn`, `info`, `debug` and
`trace`. The default baseline is `warn`, except Email's `warn,email=info`.
Non-empty `RUST_LOG` supplies the baseline instead. An explicit debug flag
selects the initial live level while preserving that baseline for restoration.
An invalid `RUST_LOG` is reported to stderr and falls back to the app's baseline;
live controls remain available. An invalid `--debug=LEVEL` is a command-line error.
The last debug flag wins. Flags after `--` are preserved for the app's parser.

```console
workshop --debug
email --debug=info
compd kms-live --debug=debug
journalctl -t workshop -f
journalctl -t email -f
journalctl -t compd -f
```

The journal identifier is always the binary name, even for an app launched
under a different Bus service name. A terminal also receives stderr output;
without a terminal, stderr is used if journald cannot be installed. A working
journal and a non-terminal stderr produce one copy, in the journal.

| Level | MixOS crates | Other dependencies | Clamped dependencies |
|---|---|---|---|
| `off` | off | off | off |
| `error` | error | error | error |
| `warn` | warn | warn | warn |
| `info` | info | info | warn |
| `debug` | debug | info | warn |
| `trace` | trace | info | warn |

The clamps cover `wgpu_core`, `wgpu_hal`, `naga`, `winit`, `calloop`, `egui`,
`eframe`, `hyper`, `tokio` and `rustls`. An explicit `RUST_LOG` baseline can use
the full filter grammar; restoring it restores its directives exactly.

Switch a running process through `<app>.debug.set`, and read it through
`<app>.debug`. These are a Bus intent and query, separate from application
actions and menus. For example, in Mix:

```mix
$body = json_encode({level: "debug"})
send workshop workshop.debug.set body=$body
send workshop workshop.debug
$body = json_encode({on: false})
send workshop workshop.debug.set body=$body
```

Set accepts exactly one of `level` or `on`. `{on:true}` selects debug;
`{on:false}` restores the recorded baseline. `{level:"off"}` disables all
tracing events; `{level:"warn"}` selects a plain warn filter, including in
Email. Both commands return `{level, on, filter, baseline}`. `on` is true
when the filter's maximum enabled verbosity is debug or trace. For a custom
baseline, `filter` is the complete authority; `level` summarises its maximum.
Unknown levels, extra fields and malformed requests are refused without
changing the active filter. A query takes an empty object.

An app running with `--service NAME` answers `NAME.debug` and
`NAME.debug.set` on that service; apps whose discovery uses canonical app
names also accept those names. compd keeps the literal command names
`comp.debug` and `comp.debug.set` under every `--bus-service` name:

```mix
$body = json_encode({level: "trace"})
send "comp-nested" "comp.debug.set" body=$body
```

Any local or mesh caller may change the level. The setting is live for this
process only: it is not saved to SQLite or app settings and resets at restart.
Daemons' durable `<service>.log` properties remain a separate namespace.

Debug events describe Bus commands and their return codes, window and
viewport lifecycle, and settings application. compd also records mapping,
placement, grab start/end and initial placement configures. Configure streams
and motion detail are trace;
debug adds no periodic or per-frame events. Disabled debug events do not
format a message or write to the journal. Emitted events cost log volume.

Under `apps@N`, all apps share a systemd unit and its journal rate limit.
A chatty app at debug or trace can therefore cause other apps' lines to be
dropped. Watch one binary with `journalctl -t <binary> -f` and restore the
baseline when finished. `tracing-journald` maps tracing DEBUG to journal
priority 6 and TRACE to 7; `-o json` exposes those priorities and event fields.

compd's separate developer logger still uses its existing `log_level`
configuration and stderr drain. The live tracing control does not change
that logger.

Pull recent events with `<app>.debug.recent` or `comp.debug.recent`. This is
a read-only citizen query, available under instance names in the same way
as `.debug`. There is no log topic: log traffic never streams over the Bus.

```mix
$body = json_encode({limit: 200, level: "debug"})
send workshop workshop.debug.recent body=$body
```

The reply is `{events, seq, next}`. Entries contain
`{seq, ts, level, target, message, fields}`; `ts` is UTC RFC 3339 and `fields`
preserves numbers and booleans. `seq` is the newest captured sequence,
including events outside the selected page. `next` is the last returned
sequence: pass it as `since` to continue without skipping a capped page.
With no `since`, the query returns the newest matching page, oldest first.
With `since`, it returns the earliest matching entries after that cursor.
`level` selects that severity and more severe events (`warn` includes errors;
`off` selects none). `limit` defaults to 200 and must be 1–1,000.

The ring exists only while the filter permits `info` or more verbose events
(including Email's info baseline). Returning to `warn`, `error` or `off`
frees it. Its sequence survives that reset. The ring drops oldest entries
at 2,000 events or a conservative 1 MB resident-size budget; expired cursors
resume at the oldest retained event. Individual text fields truncate at
4,096 UTF-8 bytes and entries retain at most 32 fields. Use journald for the
durable record. A query only reads the ring; it never enables capture.

At a plain `warn` baseline or `off`, logging starts no threads, timers or
tasks and allocates no ring. Journald opens its socket on the first admitted
event; spans alone do not open it. If that first connection fails, the sink
falls back to stderr for non-terminal runs. Disabled debug and trace sites
return at tracing's atomic maximum-level check, without building their fields.
Every reload rebuilds tracing's callsite interest and maximum-level hint in
both directions. Event sites use typed fields or lazy Display/Debug, and
frame streams stay at trace.

Tiny release builds can opt into `--features logging/release-max-level-warn`
when building a consuming package. This default-off Cargo feature enables
tracing's `release_max_level_warn` across the dependency graph: release
builds compile out info, debug and trace. It removes the live switch to
those levels, including the verbose event sites in dependencies; Bus set
requests still change the runtime filter but cannot restore compiled-out
events. Ordinary builds keep live switching. `tools/debug_gate.mix` runs
the CBC release microbenchmark and verifies the compile-time cap; its
allocator and resource tests check disabled-event allocations, helper thread
counts, lazy sockets and max-level hints.
