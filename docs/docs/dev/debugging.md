# Live application logging

Every MixOS app and compd accepts `--debug[=LEVEL]`. `--debug` means
`--debug=debug`; the levels are `off`, `error`, `warn`, `info`, `debug` and
`trace`. The default baseline is `warn`, except Email's `warn,email=info`.
`RUST_LOG`, when set, supplies the baseline instead. An explicit debug flag
selects the initial live level while preserving that baseline for restoration.
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
placement, grab start/end and actual configures. Motion detail is trace;
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
