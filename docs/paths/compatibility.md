# Default-layout compatibility fixtures

The table in `libs/paths/src/layout_tests.rs` applies main's
`32650ae5:libs/config/src/dir.rs` match arms by hand: a direct override wins,
then the whole-tree root, then uid defaults; share ignores the whole-tree root.
It does not compute expectations with a second resolver. The original ten
`dir.rs` tests and all their assertions are retained in `libs/paths/src/lib.rs`.

Each fixture runs for **both uid 0 and uid 1000**, with executable inputs
`/opt/mixos/bin/x` and `/srv/moved tree/bin/x`. That is twenty cases, asserting
all six roots. In the table, **P** is `/opt/mixos` or `/srv/moved tree`;
`prefix` is P in every case. Expected `etc`, `var`, `run` and `share` come
from main; cache is additive, using the same uid/XDG rules.

| Fixture | uid | etc | var | run | cache | share |
|---|---:|---|---|---|---|---|
| desk-isolated | 0 | `/var/lib/mixos/desk/etc` | `/var/lib/mixos/desk/var` | `/run/mixos/session/3` | `/var/cache/mixos` | `/opt/mixos/share` |
| desk-isolated | 1000 | `/var/lib/mixos/desk/etc` | `/var/lib/mixos/desk/var` | `/run/mixos/session/3` | `/home/user/.cache/mixos` | `/opt/mixos/share` |
| desk-shared | 0 | `/var/lib/mixos/vt/etc` | `/var/lib/mixos/vt/var` | `/run/mixos/session/4` | `/var/cache/mixos` | `/opt/mixos/share` |
| desk-shared | 1000 | `/var/lib/mixos/vt/etc` | `/var/lib/mixos/vt/var` | `/run/mixos/session/4` | `/home/user/.cache/mixos` | `/opt/mixos/share` |
| system-unit | 0 | `/etc/mixos` | `/var/lib/mixos` | `/run/mixos` | `/var/cache/mixos` | P`/share` |
| system-unit | 1000 | `/root/.config/mixos` | `/var/lib/mixos` | `/tmp/mixos-run` | `/root/.cache/mixos` | P`/share` |
| bare-login | 0 | `/etc/mixos` | `/var/lib/mixos` | `/run/mixos` | `/var/cache/mixos` | P`/share` |
| bare-login | 1000 | `/home/user/.config/mixos` | `/home/user/.local/share/mixos` | `/run/user/1000/mixos` | `/home/user/.cache/mixos` | P`/share` |
| bare-root | 0 | `/etc/mixos` | `/var/lib/mixos` | `/run/mixos` | `/var/cache/mixos` | P`/share` |
| bare-root | 1000 | `/root/.config/mixos` | `/root/.local/share/mixos` | `/tmp/mixos-run` | `/root/.cache/mixos` | P`/share` |

Share follows the moved executable unless explicitly pinned by `MIXOS_SHARE`.
Nothing else changes when P moves. The system-unit user case has no HOME, so
main's `/root` home fallback applies. A user run root with no absolute
XDG_RUNTIME_DIR retains main's `/tmp/mixos-run` fallback.

Inputs, with no additional variables injected:

```text
desk-isolated:
  HOME=/home/user
  MIXOS_ETC=/var/lib/mixos/desk/etc
  MIXOS_VAR=/var/lib/mixos/desk/var
  MIXOS_RUN=/run/mixos/session/3
  MIXOS_SHARE=/opt/mixos/share
  XDG_RUNTIME_DIR=/run/mixos/session/3/desktop

desk-shared:
  HOME=/home/user
  MIXOS_ETC=/var/lib/mixos/vt/etc
  MIXOS_VAR=/var/lib/mixos/vt/var
  MIXOS_RUN=/run/mixos/session/4
  MIXOS_SHARE=/opt/mixos/share
  XDG_RUNTIME_DIR=/run/mixos/session/4/desktop
  XDG_CONFIG_HOME=/var/lib/mixos/vt/home/config
  XDG_DATA_HOME=/var/lib/mixos/vt/home/data

system-unit:
  MIXOS_VAR=/var/lib/mixos

bare-login:
  HOME=/home/user
  XDG_RUNTIME_DIR=/run/user/1000

bare-root:
  HOME=/root
```

Other tests cover all original assertions, explicit tiers, empty and relative
inputs, non-UTF-8 HOME, whole-tree precedence, strict layout keys and bounds,
logical-path escapes and a real prefix/symlink move. Parser fixtures under
`libs/paths/tests/layout/` also pass through the real Mix `load_data` builtin
in `libs/mix/tests/layout_data.rs`, comparing grammar acceptance and decoded
path bytes. These Rust checks run on CBC, not on developer desktops.
