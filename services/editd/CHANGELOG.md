# editd Bus contract

## 0.3.0

- `edit.open` accepts optional initial `text` for scratch buffers, without an
  initial undo entry. Refuse `text` with `path`, preserve the text including
  a leading BOM character, and admit its bytes through the shared budget.

## 0.2.0

Transplant the `edit.*` buffer contract and retained golden fixtures from
the predecessor repository at revision
`e0297242305f3a3c3de09f1ca01e8faa771768da`. Resolve broker discovery and recovery directories through the isolated MixOS environment.
