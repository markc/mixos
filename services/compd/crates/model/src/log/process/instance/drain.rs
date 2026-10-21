// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The log drain thread: consumes the global fan-in buffer and prints each record
//! dmesg-style (elapsed-since-start) to stderr.

use crate::debug::instance::record;
use std::io::Write;

/// Drain the fan-in buffer: print dmesg-style to stderr. No history ring and no
/// fan-out to viewers: stderr is the only sink.
pub fn drain(rx: crossbeam_channel::Receiver<record::Record>) {
    let stderr = std::io::stderr();
    while let Ok(rec) = rx.recv() {
        let record::Record {
            level,
            crate_name: _,
            function,
            message,
            at,
            ack,
        } = rec;
        let elapsed = record::since_start(at);
        {
            // single writer (this thread) — `function` already carries the crate path
            let mut out = stderr.lock();
            let _ = writeln!(
                out,
                "[{:>5}.{:06}] {} {}: {}",
                elapsed.as_secs(),
                elapsed.subsec_micros(),
                level.label(),
                function,
                message,
            );
        }
        // abort! blocks on this — signal only after the record is printed.
        if let Some(ack) = ack {
            let _ = ack.send(());
        }
    }
}
