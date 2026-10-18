// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
pub mod delegate;
pub mod delegate_main {
    pub use crate::delegate::main::delegate_main::*;
}

pub mod gesture;
pub mod main;
