// SPDX-License-Identifier: MIT OR Apache-2.0
//! inspector: BusViewer's headless engine. It holds the protocol model, the
//! supervised Bus connection and the rules, and knows nothing about the UI;
//! `busviewer` renders it with egui (AGENTS.md §4.1, engine first).
pub mod bus;
pub mod engine;
pub mod model;

pub use engine::{Dialog, Effect, Engine, Row, RowKind, Session, UiState};
pub use model::{Selection, Snapshot};
