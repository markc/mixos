// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! inspector: Workshop's headless engine. It holds the protocol model, the
//! supervised Bus connection and the rules, and knows nothing about the UI;
//! `workshop` renders it with egui (AGENTS.md §4.1, engine first).
pub mod bus;
pub mod engine;
pub mod model;
pub mod workshop;

pub use engine::{Dialog, DockSide, Effect, Engine, Row, RowKind, Session, UiState};
pub use model::{Selection, Snapshot};
