// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
// The type-erased persistence handle. Deliberately serde-free: the heavy
// serialization lives in the generated `snapshot`/`rehydrate` fns (in the owning
// crate, via `persist_version!`), so this crate — which the `System` trait depends on —
// stays dependency-light.
pub mod base;
