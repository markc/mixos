// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The log drain thread: consumes the global fan-in buffer and prints each record
//! dmesg-style (elapsed-since-start) to stderr.

pub mod drain;
pub use drain::*;
