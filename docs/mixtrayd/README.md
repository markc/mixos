# mixtrayd

`mixtrayd` is the data service behind the MixOS tray popup, `mixtray`. It
registers on the noded Bus as `mixtrayd` and answers one verb,
`mixtrayd.snapshot`, with four tabs of state:

| Tab | Source |
|---|---|
| COS | systemd `ActiveState` for the MixOS units (system and user managers) |
| BUS | the services noded has registered (`noded.list`) and mesh peers (`noded.peers`) |
| MIX | the installed `mix` binary and its version |
| SSH | the concrete `Host` aliases in `~/.ssh/config` |

Each tab carries its own `error` field, so one failing source does not blank
the others. The daemon polls nothing: it reads its sources when a snapshot is
asked for.

## Live updates

The daemon subscribes to systemd's unit signals on the system and user buses
and keeps a revision that moves whenever a COS unit changes. Every snapshot
carries the revision it was read at. `mixtrayd.wait` takes a revision and
answers with a new snapshot as soon as the revision moves past it, or after
45 seconds, inside the Bus client's 60-second call limit. While the popup is
open it keeps one wait running and starts the next as each one returns, so a
unit's state changes on screen as systemd reports it.

## Control

The COS tab can start, stop and restart a unit through the `mixtrayd.control`
verb. A system unit goes through hostd's `unit.control` verb: hostd is root,
and its grant `mixtray-units` names the only units this daemon may act on, so
no sudo or polkit prompt is involved. The daemon refuses `hostd.service` and
its own unit, and it marks each unit it can control in the snapshot.

Every control action is recorded as a hostd receipt, and `hostd.undo` refuses
it, since a restart has no undo.

## Use

```
mixtrayd serve                   # the daemon (the default)
mixtrayd snapshot                # print one snapshot from the running daemon
mixtrayd wait REVISION           # print the next snapshot once a COS unit changes
mixtrayd control UNIT ACTION     # start, stop or restart one controllable unit
mixtrayd --version
```

## Not yet

- MIX scripts, runs and trash have no MixOS source yet, so the MIX tab
  reports only the binary.
- SSH keys and trash are not listed.
