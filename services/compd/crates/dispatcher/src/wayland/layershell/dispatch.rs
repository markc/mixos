// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>

pub mod wire {
    pub use crate::wayland::layershell::wire::*;
}

pub use protocols::layershell::types::{AnchorMode, OverlayPlacement};
