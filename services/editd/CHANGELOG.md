# editd Bus contract

## 0.3.0

- `editd@.service` wants the session broker instead of requiring and binding
  to it, and also orders after the node's system broker. A session that uses
  the system broker, with `noded@N` masked, now starts editd. Before, every
  start job was refused.

- Non-empty seeded scratch buffers report `dirty: true` at revision zero and
  require a save or forced close. Empty scratch buffers remain clean.

- `edit.open` accepts optional initial `text` for scratch buffers, without an
  initial undo entry. Refuse `text` with `path`, preserve the text including
  a leading BOM character, and admit its bytes through the shared budget.

## 0.2.0

Transplant the `edit.*` buffer contract and retained golden fixtures from
the predecessor repository at revision
`e0297242305f3a3c3de09f1ca01e8faa771768da`. Resolve broker discovery and recovery directories through the isolated MixOS environment.
