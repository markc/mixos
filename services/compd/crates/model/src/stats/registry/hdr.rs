// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Live HDR encode parameters (M5), set from the developer tool over gRPC and read by
//! the renderer each frame into the encode shader's uniform. A version counter lets the
//! renderer re-upload the UBO only when something changed. Field order + types mirror
//! the WGSL `Tuning` struct exactly (all f32, tightly packed).

pub mod hdr;
pub use hdr::*;
