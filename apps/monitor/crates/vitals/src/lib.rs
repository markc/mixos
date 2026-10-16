// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! vitals: Monitor's headless engine. It holds what monitord reported,
//! the graph history and the window's state, and knows nothing about the
//! UI or the Bus transport; `monitor` renders it with egui (AGENTS.md §4.1,
//! engine first).
//!
//! Monitor reads monitord's JSON, never its Rust types
//! (docs/docs/decisions/2026-10-16-monitord-and-monitor.md): [`model`]
//! holds Monitor's own lenient view of those replies.
pub mod engine;
pub mod format;
pub mod history;
pub mod model;

pub use engine::{Dialog, Effect, Engine, Fetch, Session, UiState};
pub use model::{Column, Processes, Snapshot, Tab};
