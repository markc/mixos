# Live application logging

Every MixOS app and compd accepts `--debug[=LEVEL]`. `--debug` means
`--debug=debug`; the levels are `off`, `error`, `warn`, `info`, `debug` and
`trace`. The default baseline is `warn`, except Email's `warn,email=info`
and compd's `warn,compd=info` (one-time startup diagnostics).
Non-empty `RUST_LOG` supplies the baseline instead. An explicit debug flag
selects the initial live level while preserving that baseline for restoration.
An invalid `RUST_LOG` is reported to stderr and falls back to the app's baseline;
live controls remain available. An invalid `--debug=LEVEL` is a command-line error.
The last debug flag wins. Flags after `--` are preserved for the app's parser.
Help and version requests install no logging subscriber and open no journal socket,
even with `--debug=trace` or an invalid `RUST_LOG`.

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

Fatal command-line and startup errors always print one plain
`<app>: <error>` line on stderr, including without a terminal or with logging
off. When ERROR is enabled, the `logging::failure` event also reaches the
journal with `app` and `error` fields (journal `F_APP` and `F_ERROR`), and the
recent-event ring while recording. Stderr formatting and the journal's stderr
fallback suppress that event to avoid printing the fatal error twice.

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
Email and compd. Both commands return `{level, on, filter, baseline}`. `on` is true
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

The reply is `{events, seq, next, recording}`. Entries contain
`{seq, ts, level, target, message, fields}`; `ts` is UTC RFC 3339 and `fields`
preserves numbers and booleans. `seq` is the newest captured sequence,
including events outside the selected page. `next` is the last returned
sequence: pass it as `since` to continue without skipping a capped page.
With no `since`, the query returns the newest matching page, oldest first.
With `since`, it returns the earliest matching entries after that cursor.
`level` selects that severity and more severe events (`warn` includes errors;
`off` selects none). `limit` defaults to 200 and must be 1–1,000.

The ring exists only while the live filter's maximum verbosity exceeds the
recorded baseline and is above `warn`. Every baseline, including Email's and
compd's info baselines, starts without a ring. Raising the level through
`.debug.set` or `--debug` enables recording; returning to the baseline or to
`warn`, `error` or `off` frees it. While off, recent queries return an empty
list, `recording: false` and the current sequence. The sequence survives that
reset. The deque grows on demand and drops oldest entries
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

## Test temporary files

Run commands through `mix tools/test_tmp.mix COMMAND ARG...` on a build
worker. It supplies a fresh private `TMPDIR`, counts immediate entries
(including hidden names), prints `TMP before=0 after=N`, and fails if any
ordinary entry survives. The release gate applies the same check after its
stage process cleanup. Concurrent runs have different roots.

Hold a `tempfile::TempDir` for the whole fixture lifetime, including any
background worker. A returned path needs a returned owner too. Mix fixtures
use `finally`; process fixtures acquire their kill-and-wait guard immediately
after spawning, before reading readiness. Neither unwinding nor `finally`
can recover from SIGKILL or a machine crash.

Failed runs and explicitly retained failure diagnostics belong under
`$TMPDIR/mixos-test-evidence/`. Successful ordinary fixtures are removed;
agentd integration logs and the native-mail fixture retain failed evidence.
The wrapper moves failed private roots beneath its parent's evidence root.
Operators may age out that root after collecting diagnostics; it contains
no release gate logs or receipts. Empty evidence directories do not cause
successful runs to be retained. A literal `/tmp` in a fixture bypasses the
private-root check, so creators must honour `TMPDIR`.

Release logs use `$TMPDIR/mixos-release-gate-*`. Before creating a run, the
gate keeps the newest 19 existing directories (20 including the new run),
active runs, and directories referenced by retained `.cbc-logs` receipts or
their stdout logs. A stale `active.pid` does not protect a dead process.

The November 2026 leak audit found these creator families:

| Prefix | Creator | Original survival mechanism |
| --- | --- | --- |
| `mix-*`, `mix_*` | `libs/mix/src/builtins.rs`, `libs/mix/tests/{archive,require_modules,script_path,mime_type,lint_require_calls,head_tail,member_call_extension}.rs` | Path-only helpers with no owner; some tests cleaned only after assertions |
| `workshop*` | `apps/workshop/tests/shell.rs` egui harness runtime | Directory never attached to the rig's lifetime |
| `busviewer*` | Former `apps/busviewer/tests/shell.rs`, renamed to Workshop | Same runtime fixture; historical name |
| `prefs*` | `apps/prefs/tests/{pages,shell}.rs` egui harness runtime | No cleanup; per-process paths also shared between rigs |
| `mimedb*` | `libs/mimedb/src/lib.rs::tests::mime_dir` | No cleanup, including the oversized database fixture |
| `testkit*` | `services/compd/crates/testkit/src/harness.rs::isolate_config` | Once-only process config with no final owner |
| `compd*`, `compd-doc*` | compd model preference tests, slots persistence/sdnotify tests, testkit keymap and preview fixtures | Path-only helpers or unconditional preview PNG retention |
| `scene*` | `services/compd/crates/scene-host/src/conf.rs`; panel-template Mix test | Path-only helper; panel fixture had no final cleanup |
| `mesh*` | `cli/mesh-sign/src/main.rs::tests::{scratch,signing_fixture}` | Path-only helpers without cleanup |
| `dbus*` | Toolkit chooser's private `dbus-daemon` fixture | Daemon chose a global socket; now explicitly inside an owned directory |
| `agentd*`, `agent-cli*` | `services/agentd/tests/{agentd,agentd_resume,agent_cli}_test.mix` | Failure evidence deliberately retained outside a common root |
| `noded*` | `services/noded/src/authority.rs::tests::tmpdir` | Removes the previous fixture before a test, never after it |
| `mixos-release-gate-*` | `tools/release_gate.mix` | Log retention without pruning |

`libs/test-broker/src/lib.rs::Broker` already joins its worker before removing
its root. `tests/lib/private_bus.mix` retires brokers using recorded process
identity; the calling test owns its directory. These are distinct from
path-only helpers. Historical `maild_ops*` and `compose*` names could not be
matched to the available source/history; a full filename or saved listing
is needed to attribute them rather than guess.

The 2026-11-03 CBC audit used clean private roots for every command:

| Suite | Pushed source | Passing results across two runs | Leftovers, run 1 / run 2 |
| --- | --- | --- | --- |
| mimedb, Mix, model, slots, scene-host, testkit, Preferences, Workshop | `2d26779810e0` | 5,436 | 0 / 0 |
| mesh-sign, mesh, noded, portald, toolkit | `cde61ae4574e` | 1,834 | 0 / 0 |
| guard regression, agent validation/log/CLI/service/resume, panel template | `2541f8ea3aca` | Seven scripts each run twice | 0 / 0 |

The first guarded core audit had passed its assertions but left 149 entries;
the guard rejected it. The later runs closed those leaks. The guard's own
regression deliberately creates a hidden leak and verifies rejection before
removing its fixture. Clippy passed for all 14 affected crates at
`c393fff5d75a`, using the release gate's lint policy. Formatting and lockfile
diffs came from CBC. The modified native-mail acceptance fixture compiled;
its live test remains ignored and was not exercised by this audit.
