---
title: Native node broker
description: The MixOS Bus broker, which handles service registration, discovery, routing, subscriptions, authenticated local session grants and mesh peering between nodes.
---

# Native node broker

`noded` is MixOS's ABP broker. Services register with it, discover other
services and exchange commands or topic events. Communication between nodes
uses ABP peer connections between brokers.

Native applications connect to the broker's configured Unix endpoint.
Credential checks and native session proofs identify the caller; a service
name alone does not confer authority. Mesh admission verifies signed
inventory and an unchanged cross-version admission transcript. Mesh peers
use the same frozen ABP wire and signed-inventory admission contract as
existing nodes.

The broker does not require a D-Bus session. Run `noded --version` to
inspect the package version and build provenance. noded is one of the
personalities of the [system](../system/README.md) binary.

## Configuration

Configuration is strict data in `node.conf.mix`, resolved beneath `MIXOS` or
an explicit `MIXOS_ETC`. Isolated roots never fall back to host
configuration. Set `MIXOS` to an isolated runtime root and supply
`etc/node.conf.mix`, for example:

```text
node: "example"
noded: { port: 4200, unix_socket: "<runtime>/noded/bus.sock", account: "noded" }
```

Without `noded.unix_socket`, the broker binds `<run>/noded/bus.sock` using
the environment's runtime directory (`MIXOS_RUN`, `MIXOS`, installed layout,
then the uid tier). Native clients use one shared resolver: explicit
`MIXOS_BUS_SOCKET` or caller endpoint, `noded.unix_socket` from the same node
file as the URL, environment runtime, then the endpoint advertised by
`noded.ping` on the resolved TCP URL. A process is placed in an environment
when its run root comes from `MIXOS_RUN`, `MIXOS` or a run entry in `layout.mx`.
Its only run-root candidate is its own `<run>/noded/bus.sock`. If that broker
is gone it fails naming that socket and never attaches to the host broker,
even with matching credentials.

A process using the uid tier default belongs to no session or desk. Its
node's broker is the system one, so the system `<run>/noded/bus.sock` is
appended after its own candidate. The tier default is `/run/mixos` as root,
`$XDG_RUNTIME_DIR/mixos` as a user, or `/tmp/mixos-run` without a usable XDG
runtime directory. The candidates coincide as root and are deduplicated.
An override equal to the tier default also counts as default.

Missing components (`ENOENT` or `ENOTDIR`) are skipped with a warning.
Existing candidates must pass every verification check; permissions,
symlinks, non-sockets, ownership,
peer credentials, version and refused connections are fatal. A server-PID
pin uses its endpoint alone, without skipping or ping discovery. Exhausted
searches list all paths.

Clients capture socket candidates before connecting. If a selected endpoint
disappears, an unpinned client retries once with the remaining candidates;
a second disappearance returns the missing-candidates error. Other existing
endpoint failures remain fatal. The shell's ordinary send probe disables
ping discovery so an absent local socket does not trigger TCP ping traffic.

A launch handoff (`MIXOS_SESSION_FD`) uses alone the endpoint its parent
stamped into `MIXOS_BUS_SOCKET`: the verified endpoint that issued the grant.
It uses no node socket configuration, defaults, ping discovery or TCP
fallback. A handoff without that stamp is refused, not resolved. The same
variable is a pin under the marker and a skippable override without it.
A missing issuing socket reports one connection refusal while the shell
remains usable. If Term cannot stamp a handoff, it starts the pane without a
native session and records a diagnostic.
Term's own supervisor connection retains ordinary configured discovery.

Clients trust the account selected by `MIXOS_BROKER_ACCOUNT`, then
`noded.account`, then the default `noded`. They never infer it from a socket's
owner. Account lookup errors name the account and its configuration source.
URL lookup uses the same node-file search and retains its loopback default
on read or parse failure.

An explicit socket override is not blocked by unreadable or invalid socket
settings in node.conf. Account settings are checked independently: an explicit
`MIXOS_BROKER_ACCOUNT` overrides broken account configuration, while clients
relying on `noded.account` still report configuration errors.

## Running it

Run `noded serve`; `--no-monitor --no-log` disables its optional observation
clients. The Unix broker endpoint verifies peer credentials before
accepting native session proofs. The operating system supervises the
process; application requests and node-to-node control use ABP.

The system unit runs as the `noded` system user and group, created by its
`sysusers.conf`, and pins `MIXOS_ETC`, `MIXOS_VAR` and `MIXOS_RUN`. To use
another account, override `User` and `Group` in `noded.service.d/user.conf`
and set `noded.account` in `node.conf.mix` to match. The desk's `noded@.service`
continues to run as the desk user.

## Tests

Tests cover broker commands, scope isolation, replay refusal, authority
reload, mesh admission and recovery. Run
`cargo test -p noded -p bus -p config -p props -p mesh -p mesh-trust` on a
configured build worker.

## Provenance

This source was transplanted from the noded, mesh and mesh-trust
components of MixOS at `e0297242305f3a3c3de09f1ca01e8faa771768da`.
Cryptographic domain bytes remain unchanged. Version 0.18.3 adds the
structured registration-rejection body (additive only; see the
[changelog](https://github.com/markc/mixos/blob/main/services/noded/CHANGELOG.md));
the Bus contracts otherwise match 0.18.2.

The source is
[`services/noded`](https://github.com/markc/mixos/tree/main/services/noded).

## Open Bus commands

`noded.tap` and `noded.observe.start` accept callers from every node,
including anonymous connections. Observation events are delivered on the
connection that started the subscription; registration is unnecessary.
Connection quotas, bounded queues, filtering and payload redaction still
apply. Native-session traffic can cross nodes. Broker identity stamps,
mesh admission, topic ownership and session-record ownership remain enforced.

Node config preserves unknown keys as unowned data, so existing files with
`noded.mesh_open` or `observe.allowed_services` still load. Neither key
controls broker behaviour.