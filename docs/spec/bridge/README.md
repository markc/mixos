# Bridge contract

Status: draft, version 0.1.0. Machine-checkable fixture:
[`bridge.spec.mix`](bridge.spec.mix). Verified by
`services/bridged/tests/bridge_core_test.mix` (the config contract against
the fixture, the wire protocol against a stand-in app) and
`services/bridged/tests/bridged_test.mix` (every verb over a private broker).

`bridged` puts an external application on the Bus. The application offers a
line-delimited JSON control channel on loopback TCP; one `bridged` instance
per application maps six Bus verbs onto it. This page fixes the control
channel an application must offer, the instance config schema, the serve-name
rule and the Bus verbs.

## The control channel an application offers

| Item | Rule |
|---|---|
| Transport | TCP on the loopback interface, one port per application. The bridge keeps one connection open and reuses it. |
| Framing | One JSON object per line, UTF-8, `\n`-terminated (a trailing `\r` is ignored). A line is at most `max_line` bytes. |
| Auth (`auth: "token"`) | The first line on every connection is `{"id":"auth","method":"auth","params":{"token":"<64 hex>"}}`. The application replies `{"id":"auth","ok":true,...}` or refuses and closes. The bridge sends nothing else until that reply is ok. |
| No auth (`auth: "none"`) | Requests start at once. An application that receives an `auth` line it does not expect should refuse it and close. |
| Request | `{"id":N,"method":"<name>","params":{...}}`, `N` a number the bridge increments per request. |
| Reply | `{"id":N,"ok":true,"result":<any JSON>}` or `{"id":N,"ok":false,"error":"<text>"}`, matched to the request by `id`. Lines that are not JSON or carry another `id` are skipped. A reply without a boolean `ok` is a transport failure. |
| Methods the named verbs use | `engine.commands`, `engine.execute` (`{command, params}`), `ui.inspect`, `ui.screenshot` (`{path?, focus?}`). An application without one of them answers `ok:false`; `<app>.call` reaches any other method. |

## Bus verbs

An instance started with `--name <app>` answers under its own name.

| Verb | Body | Sends | Reply on rc 0 |
|---|---|---|---|
| `<app>.call` | `{method, params?}` | `<method>` with `params` (default `{}`) | the app's `result` |
| `<app>.commands` | — | `engine.commands` | the app's `result` |
| `<app>.execute` | `{command, params?}` | `engine.execute {command, params}` | the app's `result` |
| `<app>.inspect` | — | `ui.inspect` | the app's `result` |
| `<app>.screenshot` | `{path?, focus?}`, or empty | `ui.screenshot` with only those keys | the app's `result` |
| `<app>.status` | — | nothing | `{service, connection, host, port, auth, instance, connects, failures, calls, last_error, last_latency_ms, retry_after_s}` |

`connection` is `never-connected`, `up` or `down`. `status` never contains
the token.

| rc | Meaning | Body |
|---|---|---|
| 0 | The app replied `ok:true` | its `result` as JSON |
| 10 | The app replied `ok:false` | its `error` text |
| 11 | Connect, auth, transport or timeout failure, or backing off after one | text saying which |
| 12 | A bad request from the caller (missing field, body not a JSON object) | text naming the field |

Every call runs under one deadline, `timeout_s`, covering connect, the auth
reply and the request's reply. **Writes are not bounded yet:** a peer that
stops reading can hold a request write, and the serve pump with it, for an
unbounded time until Mix 0.112.1's `tcp_send` timeout lands (pending); the
bridge will then pass the remaining deadline to every write.

After a transport failure, or a reply without a boolean `ok`, the connection is dropped and no new one is tried for
`backoff_s`; calls in that window answer rc 11 at once.

The token never appears in a reply: any occurrence of it, in any letter case,
in an app's error text, in a result, or in `last_error`, is replaced by
`[redacted]`.

**Registration is pending.** The six verbs are an instance family,
`<app>.<verb>` with owner `bridged` and runtime members. They enter
`docs/spec/bus/verbs.conf.mix` together with the verb registry gate's family
mapping (`members: "instances"`), not before.

## Instance config, version 0.1.0

A strict-data document (`load_data`; never executed).

**Path:** `$BRIDGED_CONFIG` when set; otherwise `<etc>/bridge/<app>.conf.mix`,
where `<etc>` is `$MIXOS_ETC` when set, else `/etc/mixos`. The file stem is
the serve name.

```text
port: 47801,
token_file: "/home/user/.config/fakeapp/control.token",
timeout_s: 20
```

| Key | Type | Default | Rule |
|---|---|---|---|
| `port` | integer | required | 1–65535 |
| `host` | string | `"127.0.0.1"` | `"127.0.0.1"` or `"localhost"`, which is connected to as `127.0.0.1` (never `::1`); anything else is refused |
| `auth` | string | `"token"` | `"token"` or `"none"`; an explicit `nil` is refused |
| `token_file` | string | — | required with `auth: "token"`, refused with `auth: "none"`; a leading `~/` expands to `$HOME` |
| `timeout_s` | number | 5 | (0, 300]: the whole-call deadline |
| `connect_timeout_s` | number | 2 | (0, 300]: the connect step, within the deadline |
| `backoff_s` | number | 2 | [0, 300]: the pause after a transport failure |
| `max_line` | integer | 1048576 | 64–16777216 bytes per reply line |
| `instance` | string | — | 1–64 of `[A-Za-z0-9_-]`; echoed by `status` so a launcher can recognise its own bridge |

Unknown keys are refused, so a typo never falls back to a default. The token
file holds exactly 64 hex characters (one trailing newline allowed) and has
no group or other permission bits (`chmod 600`). Errors name the file, never
its content.

## Serve name and the start checks

The name must match `^[a-z][a-z0-9-]{1,30}$`, the Bus's own rule for a
service name, and must not start with `mixos-` (`mix --serve` strips that
prefix before registering, so the checked name and the Bus name would
differ). The broker's own names are refused as noded refuses them: `noded`,
`noded-*`, and the session-name shape `<t|c><1-7 of [a-z0-9]>-<22 of
[a-z2-7]>`. It must also not be the first segment of a verb the registry
already gives to another component, so an instance can never answer, say,
`settings.status`. The registry read is `$BRIDGED_VERBS`, else the installed
`/opt/mixos/share/bus/verbs.conf.mix`:

| Registry entry | Reserves |
|---|---|
| `verbs: [{name, ...}]` | the first segment of `name`, whatever the status (retired names stay reserved) |
| `families: [{pattern, members: "instances"}]` | nothing (runtime members, such as bridged's own family) |
| `families: [{pattern, members: [names]}]` | each listed name |
| `families: [{pattern}]` with a literal first segment | that segment; an `<app>` or `*` first segment reserves nothing |

The guard fails closed: a missing or unreadable registry, one without a
`verbs` list, an entry without a `name` or `pattern`, or an unknown `members`
form refuses the start. `families` is optional, so the schema-1 registry and
later schemas with families both read.

A refused start prints `bridged: refusing to start: CODE: message` and exits
2. Codes: `NAME_INVALID`, `NAME_RESERVED`, `REGISTRY`, `CONFIG`, `TOKEN`.
`mix bridged.mix --check <app>` runs the same checks without joining the Bus
and exits 0 when the instance may start; an unexpected error at start exits 3,
never 2.

The unit runs `mix bridged.mix --start <app>`. That process runs the same
checks in process, before the name reaches the Bus, and exits 2 on a
documented refusal without starting anything. Otherwise it runs
`mix --serve bridged.mix --name <app>` as its child (no shell) and exits with
the child's exit code. Mix has no process-replacing exec, so the parent and
child pair is deliberate; the unit's control-group kill stops both. The unit
never restarts on exit 2 (`RestartPreventExitStatus=2`); every other non-zero
exit, including a missing or unreadable script (exit 1), is a failure,
restarted under `Restart=on-failure` and rate-limited by
`StartLimitBurst=5` in `StartLimitIntervalSec=60`. `$BRIDGED_MIX` overrides
the mix binary the child runs under (default `/opt/mixos/bin/mix`); it exists
for tests.

## Changes

- 0.1.0 (2026-10-10): first version.
