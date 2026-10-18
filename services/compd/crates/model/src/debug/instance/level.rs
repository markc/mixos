// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Logging frontend data types: [`Level`], [`Instance`], [`Record`], and the
//! `COMPOSITOR_LOG_LEVEL` parser. Split out of `instance.record` (the macro crate).

pub mod level;
pub use level::*;
