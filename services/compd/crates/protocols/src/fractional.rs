// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
pub mod state {
    pub use crate::fractional::emit::{NestedCompositorSurface, emit_to_surfaces};
    pub use crate::fractional::scale::{
        DebounceCycle, Fractional, FractionalScaleConfig, Published,
    };
}

pub mod config;
pub mod debounce;
pub mod emit;
pub mod scale;
