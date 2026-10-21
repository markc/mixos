// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Monitor reads monitord's JSON, not its Rust types (the monitord and
//! Monitor decision). These fixtures are real `monitord snapshot --json`
//! and `monitord ps --json --tree` output, with the host, hardware models,
//! mounts and command lines made neutral.
//!
//! Monitor's model defaults every field, so a field monitord renamed would
//! read as empty rather than fail. The pointers below are the fields the
//! window draws: each must be present by name (its value may be null where
//! monitord can't know it).
use serde_json::Value;
use vitals::model::{Processes, Snapshot};

const SNAPSHOT: &str = include_str!("fixtures/monitord-snapshot.json");
const PROCESSES: &str = include_str!("fixtures/monitord-processes.json");

fn present(doc: &Value, pointers: &[&str]) {
    for pointer in pointers {
        assert!(
            doc.pointer(pointer).is_some(),
            "monitord no longer sends {pointer}"
        );
    }
}

#[test]
fn the_snapshot_fields_monitor_draws_are_sent() {
    let raw: Value = serde_json::from_str(SNAPSHOT).unwrap();
    present(
        &raw,
        &[
            "/host",
            "/ts",
            "/uptime_s",
            "/cpu/model",
            "/cpu/cores",
            "/cpu/busy",
            "/cpu/user",
            "/cpu/system",
            "/cpu/iowait",
            "/cpu/per_core/0/cpu",
            "/cpu/per_core/0/busy",
            "/cpu/per_core/0/mhz",
            "/memory/total",
            "/memory/used",
            "/memory/available",
            "/memory/cached",
            "/swap/total",
            "/swap/used",
            "/load/one",
            "/load/five",
            "/load/fifteen",
            "/tasks/total",
            "/disks/0/name",
            "/disks/0/model",
            "/disks/0/read_bps",
            "/disks/0/write_bps",
            "/disks/0/busy",
            "/filesystems/0/mount",
            "/filesystems/0/device",
            "/filesystems/0/kind",
            "/filesystems/0/total",
            "/filesystems/0/used",
            "/net/0/name",
            "/net/0/state",
            "/net/0/rx_bps",
            "/net/0/tx_bps",
            "/net/0/rx_bytes",
            "/net/0/tx_bytes",
        ],
    );
    let s: Snapshot = serde_json::from_value(raw).unwrap();
    assert_eq!(s.host, "alpha");
    assert!(s.cpu.cores > 0 && s.cpu.per_core.len() == s.cpu.cores);
    assert!(s.memory.total > 0 && s.memory.used <= s.memory.total);
    assert!(!s.disks.is_empty() && !s.net.is_empty());
}

#[test]
fn the_process_fields_monitor_draws_are_sent() {
    let raw: Value = serde_json::from_str(PROCESSES).unwrap();
    present(
        &raw,
        &[
            "/total",
            "/count",
            "/rows/0/pid",
            "/rows/0/ppid",
            "/rows/0/name",
            "/rows/0/cmdline",
            "/rows/0/user",
            "/rows/0/cpu",
            "/rows/0/memory",
            "/rows/0/read_bps",
            "/rows/0/write_bps",
            "/rows/0/unit",
            "/rows/0/depth",
        ],
    );
    let t: Processes = serde_json::from_value(raw).unwrap();
    assert_eq!(t.rows.len(), t.count);
    assert!(t.total >= t.count);
    assert_eq!(t.rows[0].depth, Some(0), "tree order starts at a root");
}
