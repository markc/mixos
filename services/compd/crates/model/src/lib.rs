// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Shared data model: debug macros, settings and preferences, structured
//! logging, selection provenance and the statistics registry.

#[macro_use]
pub mod debug;
pub mod environment;
pub mod log;
// Selection provenance shared by the clipboard paths.
pub mod selection;
pub mod stats;
