// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! preferences: Preferences' headless engine. It holds the section registry,
//! search, Software's rows and release notes (from the `releases`
//! service), Appearance over settingsd, Apps → Email (the
//! email app's settings in settingsd, its passwords handed to the app), the
//! one-operation-at-a-time rules and the `prefs.*` Bus commands,
//! and knows nothing about the UI; `prefs` renders it with egui
//! (AGENTS.md §4.1, engine first).
pub mod appearance;
mod background;
pub mod email;
pub mod engine;
pub mod model;
pub mod navigation;
pub mod picker;
pub mod terminal;
pub mod text;
pub mod wallpaper;

pub use appearance::Look;
pub use engine::{Availability, Dialog, Effect, Engine, Op, UiState};
pub use model::{AppRow, Notes, RowState};
pub use navigation::{Category, SECTIONS, Section, SectionDescriptor, SoftwareGroup};
