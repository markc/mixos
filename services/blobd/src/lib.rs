// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>

//! `blobd` — the node blob store.
//!
//! Bytes by hash (mds's CAS), names by owner (pin rows in a blobd-owned
//! `blobd.sqlite`), movement by side-channel (the byte lane). References
//! — `{"blob":"b3:<64 hex>","size":N,"mime":…, "origin":"<node>"}` —
//! cross the Bus; bytes never do.
//!
//! The pure store core (`core`) has no Bus and no async runtime. The Bus
//! citizen (commands, events, props) is `citizen`; the HTTP byte lane is
//! `lane`; the cross-node fetch is `fetch`. The daemon `main` is ported
//! after them.

pub mod citizen;
pub mod core;
pub mod fetch;
pub mod lane;
pub mod props;
pub mod refusal;
