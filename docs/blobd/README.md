---
title: blobd
description: The node blob store: content-addressed bytes kept in mds, with owner pins, quotas and garbage collection. The MixOS port of blobd, with the store, the Bus citizen, the byte lane and the cross-node fetch.
---

# blobd

The node blob store. Bytes live by hash in the mds content-addressed store.
Names live by owner, in a blobd-owned `blobd.sqlite`. Movement between nodes
happens over a byte lane on the WireGuard address; the Bus carries references
only, never bytes.

The daemon `blobd` registers on the Bus as `blobd`, serves the `blob.*` commands,
publishes its events with `retain: false`, and listens for byte-lane transfers
on the WireGuard address. Cross-node fetches go through the same lane.

On a mesh node that runs the previous build, it replaces that build in place: the unit keeps the uid, the group
and the state root.

- **Storage:** the store runs on the `mds` library (`libs/mds`).
- **Identity kept on disk:** the cas group `blob` and the state root
  `/var/lib/mixos/blobd` are unchanged, so the port reads the live store on
  the existing nodes without moving data.
