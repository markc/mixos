// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! recorder: the screen-capture session. Ties the capture registry to the
//! compositor: the overlay UIs, the region and window capture modes, the
//! per-frame projection of the capture region, and video encoding.

#[macro_use]
extern crate model;

pub mod interface;
