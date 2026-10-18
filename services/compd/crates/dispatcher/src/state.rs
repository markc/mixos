// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
// #![feature(trait_alias)]

pub mod state;
pub use protocols::fractional::state as fractional_base;
pub mod bounds;
pub mod deferred;
