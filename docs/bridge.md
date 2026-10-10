---
title: Bridging external applications
description: How bridged puts an external application with a line-delimited JSON control channel on the Bus, one instance per application, and what that exposes.
---

# Bridging external applications

Some applications MixOS runs are not MixOS's own: they are built elsewhere
and offer their own automation port. MixOS never forks them to add Bus
verbs. Instead, `bridged` connects to an application's control channel and
answers on the Bus for it, so agents and scripts drive it the same way as
any native app.

One `bridged` instance serves one application, under the application's own
name. An instance named `fakeapp` answers `fakeapp.call`,
`fakeapp.commands`, `fakeapp.execute`, `fakeapp.inspect`,
`fakeapp.screenshot` and `fakeapp.status`.

## What the application must offer

A line-delimited JSON control channel on loopback TCP:

1. Optionally, an auth line first on every connection:
   `{"id":"auth","method":"auth","params":{"token":"<64 hex>"}}`.
2. Then one request per line, `{"id":N,"method":"...","params":{...}}`.
3. One reply per line, `{"id":N,"ok":true,"result":...}` or
   `{"id":N,"ok":false,"error":"..."}`, matched to its request by `id`.

The named verbs use the methods `engine.commands`, `engine.execute`,
`ui.inspect` and `ui.screenshot`; `<app>.call` passes any other method
through. The full contract is in the
[bridge spec](spec/bridge/README.md).

## Setting up an instance

Write the instance config, `/etc/mixos/bridge/<app>.conf.mix` (or
`$MIXOS_ETC/bridge/<app>.conf.mix` for a session). The file name is the
instance name:

```text
port: 47801,
token_file: "/home/user/.config/fakeapp/control.token"
```

For an application whose port has no auth step, write `auth: "none"` and no
`token_file`. Then start the unit:

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
- the name is the first segment of a verb already registered to another
  component, such as `settings` (`NAME_RESERVED`);
- the installed verb registry, `/opt/mixos/share/bus/verbs.conf.mix`, is
  missing or unreadable (`REGISTRY`: the check fails closed);
- the config or the token file is missing or wrong, or the token file is
  readable by group or other (`CONFIG`, `TOKEN`).

## Using it

```sh
send fakeapp fakeapp.status
send fakeapp fakeapp.commands
send fakeapp fakeapp.execute body='{"command":"file.new","params":{"width":640}}'
send fakeapp fakeapp.call body='{"method":"echo","params":{"a":1}}'
```

| rc | Meaning |
|---|---|
| 0 | The app did it; the body is its result as JSON. |
| 10 | The app refused; the body is its error text. |
| 11 | The app could not be reached: connect, auth, transport or timeout, or the bridge is backing off after one. |
| 12 | The request was wrong: a missing field, or a body that is not a JSON object. |

The bridge keeps one connection open. If the application goes away, calls
answer rc 11, the bridge waits `backoff_s` (default 2 seconds), then
reconnects and authenticates again on the next call. Every call is bounded
by `timeout_s`, writes included: an application that stops reading, or reads
too slowly, gets rc 11 at `timeout_s` and a fresh connection after the
backoff. This needs Mix 0.112.1 or later.

## Security

Read this before bridging an application.

- **Any mesh member can drive a bridged application.** The Bus gives every
  member of the mesh access to every verb; there is no per-verb access
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
- **The unit's environment is the administrator's.** Keep
  `/etc/mixos/bridge/<app>.env` and the config writable only by root:
  whoever can change them (`BRIDGED_MIX` in particular) picks the code that
  runs as `mixos`.
- **The name cannot shadow a system service.** The start checks refuse a
  name that a registered verb already starts with, and the Bus itself refuses
  a name that is already registered.

## Status

`bridged` is version 0.1.0. Its verbs are not yet in the Bus verb registry:
they are an instance family (`<app>.<verb>`, owned by `bridged`) and are
registered together with the registry gate's family support. Nothing installs
the verb registry to `/opt/mixos/share/bus/` yet either; until something
does, point `BRIDGED_VERBS` at a copy of `docs/spec/bus/verbs.conf.mix` in
`/etc/mixos/bridge/<app>.env`, or every start is refused. The component is
`services/bridged/` in the source tree.
