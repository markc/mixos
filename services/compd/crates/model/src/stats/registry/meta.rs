// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Rare diagnostics metadata (renderer kind, sync mode, output/mode, VRR/HDR flags,
//! env): lives behind a mutex updated at setup/transition time.

pub mod meta;
pub use meta::*;
