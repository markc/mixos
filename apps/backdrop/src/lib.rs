// SPDX-License-Identifier: MIT OR Apache-2.0
//! Desktop background host and Bus-driven views.
pub mod control;
pub mod program;
pub mod views;
/// Maximum accepted wallpaper document size.
pub const MAX_DOCUMENT: u64 = 64 * 1024;
