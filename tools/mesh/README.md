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
desk target paths without contacting a host or writing anything. Optional
`--probe` uses the existing native SSH/Mix path and a namespace stdin pipe to
query only noded.info and noded.peers as the session account. It requires an
absolute `MESH_MIX` path. Process success is reported separately from Bus
reply codes; neither is an admission or deployment verdict. The version1
schema has `project` and a nonempty `targets` list. Each target has `node`,
`host` (an SSH alias) and `kind` (`system` or `desk`). A desk also names its
`machine`, `account`, integer `session` from1 to6, and `profile` as one direct
child of `/var/lib/<project>`. See `tests/targets.conf.mix`.

The model distinguishes the host SSH endpoint from the admitted node and the
container namespace. It derives profile-local genesis, seed, inventory and
anti-rollback baseline paths and `noded@N.service`. The distributor integration
is currently parked behind `desk_ops.ROLLOUT_READY=false`; every new live desk
operation refuses, and the preview still reports mutation unsupported. Existing
read-only probes remain available. Never use a path-only override to provision
a desk. The guard must stay closed until the complete integration is reviewed.
Run `mix tests/targets_test.mix` for the hermetic model checks.

`_bin/name_gate.mix` fails when a project or site name appears as a literal in
this tree. Run it before a merge.

The pure `stdin_job` helper describes namespace invocation and profile context.
It separates source/bindings from process argv and distinguishes administrator
from session-account jobs. It performs no execution and does not enable a
distributor write path. Its namespace stdin, account and profile boundaries
are covered by hermetic tests.

## Guarded desk distributor draft

Both `mesh_sync` and `mesh_d2_seed` now consume the same optional private
`$MESH_HUB/_etc/mesh_targets.conf.mix`. Without it, existing system-node dispatch
is preserved. When present it must describe **every** fleet roster node exactly
once; system entries retain `host == node`, and desk entries explicitly state
`admission` (`off`, `observe` or `enforce`) and boolean `mesh_open`. Omission or
malformation cannot silently fall back to system paths. For example:

```mix
{schema_version: 1, project: "example", targets: [
  {node: "hub", host: "hub", kind: "system"},
  {node: "desktop", host: "host", kind: "desk", machine: "desktop",
   profile: "/var/lib/example/desktop-bus", session: 3, account: "desktop",
   admission: "observe", mesh_open: true}
]}
```

This describes prepared targets; it does not create accounts, profiles, units,
network routes or membership. Use a **fresh dedicated noded profile**, preserving
the desk's old anchor/baseline and all app/settings data. Profile, etc, etc/noded
and var directories must be root:session-group0750; var/noded must be
session-account:session-group0700. Existing distributor-owned files must be
root:session-group0640. A baseline, when present, stays account-owned0600 and is
never copied, deleted or reset by these tools. Root-controlled ancestors and
native binaries are mandatory.

The namespace-local `noded@N.service` must retain one native `Type=simple` start,
`KillMode=control-group`, its session runtime directory, and the declared account.
Its **last mandatory** EnvironmentFile is the root-owned0640
`<profile>/etc/noded/service.env`, containing only these literal assignments:

```text
EXAMPLE_ETC=/var/lib/example/desktop-bus/etc
EXAMPLE_VAR=/var/lib/example/desktop-bus/var
EXAMPLE_NODE_CONFIG=/var/lib/example/desktop-bus/etc/node.conf.mix
EXAMPLE_RUN=/run/example/session/3
EXAMPLE_BROKER_ACCOUNT=desktop
EXAMPLE_NODED_URL=
```

The empty broker URL prevents inherited host/desk overrides; native clients read
the explicit node configuration. That root-owned0640 config must match the
authenticated node, mesh, WG address/port, roster path, Unix socket and explicit
policy. Service environment, static executable/argv, runtime uid/environment and
binary hashes are checked independently. Runtime pid/timestamp fields do not
change the static unit fingerprint. Installed native Mix must expose all needed
filesystem-event and transaction primitives; no alternate interpreter is used.

Seeds travel as data through native SSH/Mix stdin and a namespace pipe, never
source argv or a second broker. `mesh_d2_seed` keeps its verified-inventory,
coherent secrets snapshot and current-public-key checks; it writes a root-owned
seed provenance receipt and **never restarts**. The receipt is deployment
provenance, not an independent authority: `mesh_sync` checks its seed hash/key
against the current authenticated D2 credentials before installing inventory.
Existing system seed dispatch retains its private file transport.

The worker snapshots preimages in protected `<profile>/.mesh-rollout/<token>`
directories, records absent files, uses exclusive staging beside destinations,
rechecks preimages and verifies publication. Failure logs identify the proposed
receipt path even if transport dies. Seed/roster preimages can be restored only
if no concurrent update occurred; once inventory was published, recovery requires
a higher signed epoch. The existing single-writer proposal, hubs-last order and
canonical-cache rules remain with the owning distributors.

Desk restart waits on native filesystem events for its new broker socket, with
a whole-operation deadline, not polling. A fresh session-account interpreter
checks local node/WG identity and verified mesh/epoch/hash. This is local rollout
verification; production acceptance still requires addressed cross-node ABP and
the separately reviewed WG, service/client environment, canary and rollback plan.

Run `tests/desk_ops_test.mix` for real native filesystem failure/restoration
fixtures and `tests/desk_flow_test.mix` for typed executor order/failure, unit
binding and generated-source/RPC fixtures. Neither contacts production SSH,
services or secrets. A real disposable namespace exercise remains a separate
proof requirement before claiming deployed readiness.
