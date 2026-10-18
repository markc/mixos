// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
// One world-start rehydration call: a world's single-value slots (per-world files)
// and its document tables (per-record, partition-filtered). Keeps World::build's
// hook a single call.
pub mod base;
