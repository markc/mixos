# Mesh tooling

Generators, signers and deploy scripts for a WireGuard mesh with a Bus broker
on each node. Nothing here names a project or a site: every name comes from
configuration, so a fork can relabel the whole tree.

## Configuration

| Setting | Where | What it names |
|---|---|---|
| `MESH_PROJECT` or `_etc/mesh/project.mix` | env or hub | the project: `/etc/<name>/`, `/var/lib/<name>/`, `/opt/<name>/bin/`; binaries, units and groups keep their plain names (`noded`, `mesh-sign`) |
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

- `mesh-sign` signs the inventory and prints its public key.
- `noded` is the broker each node runs.

Both are found through `project_bin()` and `installed_bin()` in `deploy_lib.mix`.

## Checks

`mix _bin/mesh_targets.mix TARGETS.conf.mix` previews explicit system and
desk target paths without contacting a host or writing anything. The version1
schema has `project` and a nonempty `targets` list. Each target has `node`,
`host` (an SSH alias) and `kind` (`system` or `desk`). A desk also names its
`machine`, `account`, integer `session` from1 to6, and `profile` as one direct
child of `/var/lib/<project>`. See `tests/targets.conf.mix`.

The model distinguishes the host SSH endpoint from the admitted node and the
container namespace. It derives profile-local genesis, seed, inventory and
anti-rollback baseline paths and `noded@N.service`. This is a read-only
foundation: existing distributors do not yet consume desk descriptors. Desk
mutation is refused until path, namespace, owner, staging, restart and proof
are integrated together. Never use a path-only override to provision a desk.
Run `mix tests/targets_test.mix` for the hermetic model checks.

`_bin/name_gate.mix` fails when a project or site name appears as a literal in
this tree. Run it before a merge.

The pure `stdin_job` helper describes namespace invocation and profile context.
It separates source/bindings from process argv and distinguishes administrator
from session-account jobs. It performs no execution and does not enable a
distributor write path. Its namespace stdin, account and profile boundaries
are covered by hermetic tests.
