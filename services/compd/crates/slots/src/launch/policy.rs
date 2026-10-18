// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Compile-time launch policy. Three orthogonal toggles select how apps are
//! spawned and managed; the defaults reproduce the historical behaviour byte
//! for byte so the new machinery can be bisected against it.

pub mod policy;
