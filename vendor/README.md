# vendor/

Patched upstream crates the workspace builds against. Each directory is kept
intact as imported, with its own licence files, which is why the root
`Cargo.toml` lists `vendor` under `[workspace] exclude`. Workspace crates reach
them through `[patch.crates-io]`.

Each crate's local edits against its upstream base are listed in that crate's
`PATCHES.md`, with the guard that fails if a re-vendor drops an edit.

| Crate | Dir | Upstream version | Wired via | Local edits | Notes |
|---|---|---|---|---|---|
| msedit (Microsoft Edit buffer) | `msedit/` | revision `826b4c097b6f14ba0a846dc56f2f0223a3aaf73a` | `#[path]` include from `libs/edit` | see `msedit/PATCHES.md` | MIT; its `lsh` and `stdext` workspaces are excluded |
| dirs-sys | `dirs-sys/` | crates.io 0.5.0, `8bcd4aa2c35990d57a2cff2953793525fc42709c` | `[patch.crates-io]` | replace one OptionExt comparison with standard Option equality; remove option-ext | MIT OR Apache-2.0; upstream XDG tests and licence gate |
