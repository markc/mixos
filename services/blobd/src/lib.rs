// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>

//! `blobd` — the node blob store.
//!
//! Bytes by hash (mds's CAS), names by owner (pin rows in a blobd-owned
//! `blobd.sqlite`), movement by side-channel (the byte lane). References
//! — `{"blob":"b3:<64 hex>","size":N,"mime":…, "origin":"<node>"}` —
//! cross the Bus; bytes never do.
//!
//! This is the port's first stage: the pure store core, with no Bus, no
//! async runtime and no daemon. The citizen layer (verbs, events, props),
//! the HTTP byte lane and the cross-node fetch follow in later stages.

pub mod core;
