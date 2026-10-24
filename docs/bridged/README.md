---
title: Bridging external applications
description: How bridged puts an external application with a line-delimited JSON control channel on the Bus, one instance per application, and what that exposes.
---

# Bridging external applications

Some applications MixOS runs are not MixOS's own: they are built elsewhere
and offer their own automation port. MixOS never forks them to add Bus
commands. Instead, `bridged` connects to an application's control channel and
answers on the Bus for it, so agents and scripts drive it the same way as
any native app.

One `bridged` instance serves one application, under the application's own
name. An instance named `fakeapp` answers `fakeapp.call`,
`fakeapp.actions`, `fakeapp.execute`, `fakeapp.inspect`,
`fakeapp.screenshot` and `fakeapp.status`.

## What the application must offer

A line-delimited JSON control channel on loopback TCP:

1. Optionally, an auth line first on every connection:
   `{"id":"auth","method":"auth","params":{"token":"<64 hex>"}}`, or with
   `auth: "key"`, the same line with `params.key`. A key is written by the
   application to a file after it starts, and may be rotated; the bridge
   reads that file on every connect.
2. Then one request per line, `{"id":N,"method":"...","params":{...}}`.
3. One reply per line, `{"id":N,"ok":true,"result":...}` or
   `{"id":N,"ok":false,"error":"..."}`, matched to its request by `id`.

The named commands use the methods `engine.commands`, `engine.execute`,
`ui.inspect` and `ui.screenshot`; `<app>.call` passes any other method
through. The full contract is in the
[bridge spec](../docs/spec/bridge/README.md).

## Setting up an instance

Write the instance config, `/etc/mixos/bridge/<app>.conf.mix` (or
`$MIXOS_ETC/bridge/<app>.conf.mix` for a session). The file name is the
instance name:

```text
port: 47801,
token_file: "/home/user/.config/fakeapp/control.token"
```

For an application whose port has no auth step, write `auth: "none"` and no
`token_file`. For an application that writes its own key file after it
starts, write `auth: "key"` with the same `token_file` (the key file's path).
The instance starts before the key exists; calls answer rc 11 until it does.
Then start the unit:

```sh
systemctl start bridged@fakeapp
```

The unit runs `mix bridged.mix --start fakeapp`. It checks the instance first,
before the name joins the Bus, then runs `mix --serve bridged.mix --name
fakeapp` as its child. A refused start exits 2, with the reason in the
journal, and is never restarted. Any other failure (a crash, a broken
install) is restarted a few times and then left failed. A normal
`systemctl stop` ends the bridge with status 143, which the unit counts as
clean. Run the bridge under the unit, not `--start` by hand: outside systemd,
killing the `--start` process alone can leave the serve child running. A
start is refused when:

- the Mix binary is older than 0.112.1, which cannot bound writes (`RUNTIME`);
- the name is not a valid Bus service name, or starts with `mixos-`
  (`NAME_INVALID`);
- the name belongs to the Bus broker (`noded`, `noded-*`) (`NAME_RESERVED`);
- the name is the first segment of a command already registered to another
  component, such as `settings` (`NAME_RESERVED`);
- the installed command registry, `/opt/mixos/share/bus/commands.conf.mix`, is
  missing or unreadable (`REGISTRY`: the check fails closed);
- the config or the token file is missing or wrong, or the token file is
  readable by group or other (`CONFIG`, `TOKEN`).

## Using it

```sh
send fakeapp fakeapp.status
send fakeapp fakeapp.actions
send fakeapp fakeapp.execute body='{"command":"file.new","params":{"width":640}}'
send fakeapp fakeapp.call body='{"method":"echo","params":{"a":1}}'
```

| rc | Meaning |
|---|---|
| 0 | The app did it; the body is its result as JSON. |
| 10 | The app refused; the body is its error text. |
| 11 | The app could not be reached: connect, auth, transport or timeout, or the bridge is backing off after one. |
| 12 | The request was wrong: a missing field, a body that is not a JSON object, or a request longer than `max_line` (1 MiB by default). |

The bridge keeps one connection open. If the application goes away, calls
answer rc 11, the bridge waits `backoff_s` (default 2 seconds), then
reconnects and authenticates again on the next call. Each call's network
work (connect, auth, writing the request, reading the reply) shares one
`timeout_s` budget, so an application that stops reading, or reads too
slowly, gets rc 11 at about `timeout_s` and a fresh connection after the
backoff. Preparing the request (encoding a large body) cannot be cut short,
so it can add to the time a call takes beyond `timeout_s`; the network step
after it then fails at once. This needs Mix 0.112.1 or later.

## Security

Read this before bridging an application.

- **Any mesh member can drive a bridged application.** The Bus gives every
  member of the mesh access to every command; there is no per-command access
  control, and `bridged` adds none. Whatever the application's control
  channel allows, any mesh member can do through `<app>.call`: send it UI
  input, run its commands and, if it has file commands, read and write files.
  Bridge only applications you would let every mesh member drive.
- **File access is the application's to limit.** An application that offers
  file I/O over its control channel should confine it to automation roots it
  is started with (read and write directories). The bridge passes paths
  through unchanged and cannot confine them.
- **The token stays on this node.** It is read from `token_file` at start
  and sent only on the loopback connection. It never appears in `status`, in
  an error or in a log line; if the application echoes it in an error or a
  result, the bridge replaces it with `[redacted]`. The token file must be
  `0600` (a file group or other can read is refused) and readable by the
  unit's user (`mixos`).
- **Loopback only.** The config refuses any host but `127.0.0.1` or
  `localhost`, and connects to `127.0.0.1` for either.
  An `auth: "none"` port is open to every local process while the
  application runs, Bus or no Bus; open it only for applications you bridge.
- **A local process can squat the port.** If the application dies, any
  local process that binds its port before it comes back receives the
  token on the bridge's next reconnect, and can then pass itself off as the
  application to the bridge, and use the token against the real application
  once it returns. Loopback TCP has no way to prove who is listening; a
  unix-socket control channel, checked by owner, would close this, and is a
  possible future protocol option. Until then, bridge applications only on
  machines whose local users you trust.
- **The unit's environment is the administrator's.** Keep
  `/etc/mixos/bridge/<app>.env` and the config writable only by root:
  whoever can change them (`BRIDGED_MIX` in particular) picks the code that
  runs as `mixos`.
- **The unit is sandboxed.** It runs without capabilities or new
  privileges, sees the file system read-only (homes included) with private
  `/tmp` and `/var/tmp`, and may send IP traffic to loopback only. So keep
  the config and the token file out of `/tmp` and `/var/tmp`, where the unit
  cannot see them. A Bus that is not on loopback (for example a session
  broker at `ws://<container-ip>:<port>`) needs a drop-in, `systemctl edit
  bridged@<app>`, adding `IPAddressAllow=<broker-ip>` under `[Service]`.
  The loopback-only IP filter is cgroup BPF: systemd enforces it for this
  system unit and the serve child on kernels and hosts that support it, and
  ignores it where they do not (some container managers, for example). The
  `systemd-analyze security` score is computed from the unit file and does
  not prove the filter is enforced on a given host.
- **The name cannot shadow a system service.** The start checks refuse a
  name that a registered command already starts with, and the Bus itself refuses
  a name that is already registered.

## Status

`bridged` is version 0.2.0 (unreleased); 0.2.0 adds `auth: "key"`. Its six commands are declared in the Bus
command registry, `docs/docs/spec/bus/commands.conf.mix` (schema 3), as the `bridged`
instance family `<app>.<command>`. The static registry gate checks the family's
owner and suffix. Installing the registry is a separate prerequisite: the
scripts and the registry must come from the same revision, and the registry
must be at `/opt/mixos/share/bus/commands.conf.mix` or named by `BRIDGED_COMMANDS`
in `/etc/mixos/bridge/<app>.env`. Otherwise every start is refused. This
source tree is not a live deployment, and nothing in it installs the
registry for you.

The family is declared with `members: "instances"`, so the running
instances are not listed. `tools/command_registry.mix --instances DIR` checks
only the private file-stem naming and collisions; it reads no config token.
Mesh access is unchanged: the Bus still gives every member every command. The
runtime guard reads the commands and families lists. It does not enforce the
full canonical format (field order, spacing); the static gate does that.

**Command classes.** `<app>.call` and `<app>.execute` are `intent`;
`<app>.actions`, `<app>.inspect` and `<app>.status` are `query`.
`<app>.screenshot` is `intent`: its `focus` option can bring the window
forward, a change another caller sees, so it is not a query even though it
only captures. See the
[instance command families decision](../docs/decisions/2026-10-13-instance-verb-families.md).

## Source and tests

`bridged` is a Mix component
([decision](../docs/decisions/2026-10-10-mix-components.md)): no Cargo
package, identity in `component.mx`. The source is
[`services/bridged`](https://github.com/markc/mixos/tree/main/services/bridged):

| Path | What |
|---|---|
| `scripts/lib/bridge_core.mix` | the stateless core: config, token, serve-name guard, line-JSON client |
| `scripts/bridged.mix` | the Bus service (`mix --serve … --name <app>`); `--start <app>` (the unit: checks, then the service as a child); `--check <app>` (the checks alone) |
| `units/bridged@.service` | one unit per application; config `/etc/mixos/bridge/<app>.conf.mix` |
| `tests/lib/fake_app.mix` | a stand-in application on `tcp_listen` (token, key or no auth, slow and dropping methods) |
| `tests/bridge_core_test.mix` | offline: config, token, guard, spec fixture, the client against stand-in apps |
| `tests/bridged_test.mix` | every command over a private broker, the start refusals, `--start` (no child on a refusal, the child's exit code passed through), the app dying and coming back |

Run the tests from the checkout root with Mix 0.112.1 or later. The mix
binary the tests run under is `BRIDGED_MIX`, else `MIXOS_TEST_MIX`, else
`/opt/mixos/bin/mix`. The Bus test needs a `noded`: `--noded <path>`, else
`MIXOS_TEST_NODED`, else `/opt/mixos/bin/noded`. An empty variable counts as
unset. `tools/mix_tests.mix` sets `MIXOS_TEST_MIX`, `MIXOS_TEST_NODED` and
`BRIDGED_MIX` to the fresh build:

```sh
mix services/bridged/tests/bridge_core_test.mix
mix services/bridged/tests/bridged_test.mix
```
