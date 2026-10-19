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
asked for, and the popup asks only when it opens.

## Use

```
mixtrayd serve       # the daemon (the default)
mixtrayd snapshot    # print one snapshot from the running daemon
mixtrayd --version
```

## Not yet

- MIX scripts, runs and trash are cosmix hub features with no MixOS source
  yet, so the MIX tab reports only the binary.
- SSH keys and trash are not listed.
