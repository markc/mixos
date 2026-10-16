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
noded: { port: 4200, unix_socket: "/run/mixos/noded/bus.sock" }
```

## Running it

Run `noded serve`; `--no-monitor --no-log` disables its optional observation
clients. The Unix broker endpoint verifies peer credentials before
accepting native session proofs. The operating system supervises the
process; application requests and node-to-node control use ABP.

## Tests

Tests cover broker verbs, scope isolation, replay refusal, authority
reload, mesh admission and recovery. Run
`cargo test -p noded -p bus -p config -p props -p mesh -p mesh-trust` on a
configured build worker.

## Provenance

This source was transplanted from the noded, mesh and mesh-trust
components of Cosmix at `e0297242305f3a3c3de09f1ca01e8faa771768da`.
Cryptographic domain bytes remain unchanged. Version 0.18.3 adds the
structured registration-rejection body (additive only; see the
[changelog](https://github.com/markc/mixos/blob/main/services/noded/CHANGELOG.md));
the Bus contracts otherwise match 0.18.2.

The source is
[`services/noded`](https://github.com/markc/mixos/tree/main/services/noded).
