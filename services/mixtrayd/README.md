# mixtrayd

A Mix Bus citizen providing the tray's snapshots, revision waits, unit control
and SSH host actions. See the [manual](../../docs/mixtrayd/README.md).

Runtime edge programs: `systemctl`, `busctl`, `ssh`, `ssh-keygen` and `ln`.
The user unit runs `mix --serve`; the layout renders the `mixtrayd` CLI into
`bin/`. Tests use an owned private broker and temporary SSH trees.
