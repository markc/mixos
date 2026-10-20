---
title: blobd
description: The node blob store: content-addressed bytes kept in mds, with owner pins, quotas and garbage collection. Ported from cosmix-blobd; the store core is ported, the Bus and byte-lane layers follow.
---

# blobd

The node blob store. Bytes live by hash in the mds content-addressed store.
Names live by owner, in a blobd-owned `blobd.sqlite`. Movement between nodes
happens over a byte lane on the WireGuard address; the Bus carries references
only, never bytes.

This is the first stage of the port: the store core, with no Bus, no daemon
and no HTTP. The citizen layer (verbs, events, props), the byte lane and the
cross-node fetch are later stages.

- **Storage:** the store runs on the `mds` library (`libs/mds`).
- **Identity kept on disk:** the cas group `cosmix-blob` and the state root
  `/var/lib/cosmix/blobd` are unchanged, so the port reads the live store on
  the existing nodes without moving data.
