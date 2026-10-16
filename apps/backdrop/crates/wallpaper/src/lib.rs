// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! backdrop's wallpaper engine: pure data, no Bus and no window.
//!
//! - [`schema`][]: the `schema: 1` document (`backdrop.mx`, the Bus body of
//!   `backdrop.set`, the command line's default). Every entry point validates
//!   through [`Config::from_document`], so a value means the same whichever
//!   way it arrived.
//! - [`colour`][]: `#RRGGBB`.
//! - [`fit`][]: crop, contain, native centre, repeat and stretch placement.
//! - [`decode`][]: PNG and JPEG under explicit limits, applied before decode.
//! - [`engine`][]: revisions and pure slideshow schedules. A request applies
//!   after its selected images decode; until then the last accepted one stays.

pub mod colour;
pub mod decode;
pub mod engine;
pub mod fit;
pub mod schema;

pub use colour::Rgb;
pub use engine::{Decode, Engine, Staged, Status};
pub use schema::{Config, Document, Fill, Invalid, SCHEMA, Wallpaper};
