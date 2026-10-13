// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! annotation: screenshot's headless engine (AGENTS.md §4.1, engine first).
//!
//! - [`document`]: the immutable captured raster and its editable objects,
//!   with stable ids, bounds and undo/redo; edits are geometry, never pixels;
//! - [`paint`]: one painter for preview, clipboard and export, so what is
//!   saved is what was shown; text is shaped with a [`Face`] the caller
//!   supplies, so no host font is ever consulted;
//! - [`viewport`]: the one image-to-view transform for drawing and pointer
//!   mapping;
//! - [`library`]: where shots are kept and published, how they are named,
//!   and the record of what was saved.
//!
//! Transplanted from `apps/cap/src/{document,viewport,capture}.rs`
//! (markc/mixos-iced); the painter no longer reaches into a UI toolkit for
//! its text shaper.
pub mod document;
pub mod library;
pub mod paint;
pub mod viewport;

pub use document::{Crop, Document, Kind, MAX_OBJECTS, MAX_PIXELS, Object, Point, Shape};
pub use paint::Face;
pub use viewport::Viewport;
