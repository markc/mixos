# mixtrayd

`mixtrayd` is the data service behind the MixOS tray popup, `mixtray`. The
popup is a KDE Plasma plasmoid in
[markc/plasma](https://github.com/markc/plasma); a native MixOS desktop
version is planned. `mixtrayd` registers on the noded Bus as `mixtrayd` and answers one verb,
`mixtrayd.snapshot`, with four tabs of state:

| Tab | Source |
|---|---|
| COS | systemd `ActiveState` for the MixOS units (system and user managers) |
| BUS | the services noded has registered (`noded.list`) and mesh peers (`noded.peers`) |
| MIX | the installed `mix` binary and its version |
| SSH | host files in `~/.ssh/hosts/`, their trash, keys in `~/.ssh/keys/`, and probe results |

Each tab carries its own `error` field, so one failing source does not blank
the others. The daemon polls nothing: it reads its sources when a snapshot is
asked for.

## Live updates

The daemon subscribes to systemd's unit signals on the system and user buses
and watches `~/.ssh/config`, `~/.ssh/hosts/` and `~/.ssh/keys/`. It keeps a
revision that moves whenever a COS unit changes, one of those files changes,
or an SSH probe starts or finishes. Every snapshot
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

## SSH hosts

Each host is one file in `~/.ssh/hosts/`, named after its alias and holding a
single `Host` line, which `~/.ssh/config` pulls in with
`Include ~/.ssh/hosts/*`. Dotfiles in that directory are shared snippets, not
hosts. The `mixtrayd.ssh` verb takes an `action`:

| Action | Fields | Effect |
|---|---|---|
| `probe` | `ids` (empty for every usable host) | runs `ssh ALIAS true` in batch mode, four at a time, 10 s each |
| `create` | `name`, `hostname`, `port`, `user`, `key` | writes a new owner-only host file using `~/.ssh/keys/KEY`; never overwrites one |
| `trash` | `id` | renames the file `.trashed-ID` |
| `restore` | `id` | renames it back, refusing to replace a live host |
| `purge` | `id` | deletes a trashed host; a live host must be trashed first |

The popup opens Connect (`konsole -e ssh ALIAS`) and Edit (`xdg-open` on the
host file) itself, since those run in the desktop session, not the daemon's.

## Use

```
mixtrayd serve                   # the daemon (the default)
mixtrayd snapshot                # print one snapshot from the running daemon
mixtrayd wait REVISION           # print the next snapshot once a COS unit changes
mixtrayd control UNIT ACTION     # start, stop or restart one controllable unit
mixtrayd ssh probe [ID...]       # test every usable host, or the ones named
mixtrayd ssh create NAME HOSTNAME PORT USER KEY
mixtrayd ssh trash|restore|purge ID
mixtrayd --version
```

## Not yet

- MIX scripts, runs and trash have no MixOS source yet, so the MIX tab
  reports only the binary.
