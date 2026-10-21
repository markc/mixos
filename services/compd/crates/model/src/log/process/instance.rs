// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! compositor.developer structured logging — **backend runtime**.
//!
//! One drain thread consumes the global fan-in buffer and prints each record
//! dmesg-style (`process.instance.drain`). Started once by
//! `crate::log::process::main::spawn`.
//!
//! There is no gRPC log stream: layer 0 has no tokio/tonic/prost and no
//! build-time protobuf step. The live log surface is journald (stderr) today and
//! the Bus later.

use crate::debug::instance::record;

/// Start the drain thread. Call exactly once.
pub fn start(rx: crossbeam_channel::Receiver<record::Record>) {
    let _ = std::thread::Builder::new()
        .name("compd-log-drain".into())
        .spawn(move || crate::log::process::instance::drain::drain(rx));
}

pub mod drain;
