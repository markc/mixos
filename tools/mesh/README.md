# Mesh tooling

Generators, signers and deploy scripts for a WireGuard mesh with a Bus broker
on each node. Nothing here names a project or a site: every name comes from
configuration, so a fork can relabel the whole tree.

## Configuration

| Setting | Where | What it names |
|---|---|---|
| `MESH_PROJECT` or `_etc/mesh/project.mix` | env or hub | the project: binaries `<name>-<tool>`, `/etc/<name>/`, `/var/lib/<name>/`, the unit `<name>-noded`, the group `<name>-noded` |
| `MESH_HUB` | env (required) | the directory that holds `_etc/mesh/` (inventory, epoch, zones) |
| `MESH_SRC` | env (required) | the project's source checkout, for built binaries |
| `MESH_BIN_DIR` | env, optional | a directory of built binaries, overriding `target/release` |
| `MESH_PREFIX` | env, optional | the installed prefix, default `/opt/<name>` |
| `MESH_MIX` | env, optional | the `mix` used for child processes, default the one on PATH |

`_etc/mesh/project.mix` is the hub's copy of the project name:

```
{project: {name: "example"}}
```

Each tool that needs a mesh name reads its own `_etc/<tool>.conf.mix`
(`expected_mesh`). The hub copies them from the `.example` files, which this
tree does not ship active.

## Layout

- `_bin/` the tools. Each is run as `mix _bin/<tool>.mix` with the variables above set.
- `_lib/deploy_lib.mix` the shared helpers: names, roots, roster, versions.
- `_lib/mesh_lib.mix` the inventory validator, the zone and WireGuard generators, the write lock.

## External binaries

The tools call two binaries built by the project itself:

- `<name>-mesh-sign` signs the inventory and prints its public key.
- `<name>-noded` is the broker each node runs.

Both are found through `project_bin()` and `installed_bin()` in `deploy_lib.mix`.

## Checks

`_bin/name_gate.mix` fails when a project or site name appears as a literal in
this tree. Run it before a merge.
