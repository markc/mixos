// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! preferences: Prefs' headless engine. It holds the panels, the
//! Applications panel's rows and release notes (from the `releases`
//! service), the Appearance panel over settingsd, the one-operation-at-a-time rules and the `prefs.*` Bus verbs,
//! and knows nothing about the UI; `prefs` renders it with egui
//! (AGENTS.md §4.1, engine first).
pub mod appearance;
mod background;
pub mod engine;
pub mod model;
pub mod navigation;
pub mod picker;
pub mod terminal;
pub mod wallpaper;

pub use appearance::Look;
pub use engine::{Availability, Dialog, Effect, Engine, Op, UiState};
pub use model::{AppRow, Notes, Panel, RowState};
pub use navigation::Section;
