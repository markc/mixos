// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! headless: compd's render-node backend with a virtual output and no seat.
//!
//! GBM/EGL owns only a render node. Frames are full repaints into two
//! offscreen textures, paced by an on-demand calloop timer at at most 60 Hz.

#[macro_use]
extern crate model;

mod frame;
pub mod wire;
