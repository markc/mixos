// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Unified logging surface for every MixOS binary.
//!
//! Scope: CLI flags (`LogOpts`), per-binary defaults (`LogDefaults`),
//! sinks (stderr / rolling file / journald), filter parsing, JSON +
//! human formats, the `--log-level none` → `EnvFilter::off()` path, a
//! live-reload handle (`LogReloadHandle`), and the full stats recorder
//! / JSONL sink subsystem.
//!
//! The core works without the Bus. A property watcher can drive live
//! filter swaps through the public `LogReloadHandle`.
//!
//! Three optional features layer on the core:
//! - `bus-handlers` — the `<svc>.stats.snapshot` Bus command handler
//!   (`stats::handle_snapshot_bus`); uses `bus::native_client`.
//! - `prometheus` — the per-daemon `/metrics` endpoint.
//! - `props` — a durable `<svc>.log` namespace and live filter watcher.
//!
//! # Hard rules
//!
//! - **stdout is reserved for protocol output**, never logs. All sinks
//!   write to stderr / files / journald.
//! - **`--log-level none` is first-class.** It installs
//!   `EnvFilter::off()` behind a reload handle so a later filter swap
//!   (or `RUST_LOG`) can raise it without restarting the binary.

pub mod debug;
/// Seam diagnostics for consumers that already use the logging core.
pub use tracing::{debug, trace};
mod defaults;
mod handle;
mod init;
mod journal;
mod opts;
#[cfg(feature = "prometheus")]
mod prometheus_attach;
mod recent;
mod sink;
pub mod stats;

pub use defaults::{LogDefaults, RotationMode};
pub use handle::{LogError, LogHandle, LogReloadHandle};
pub use init::{default_log_dir, init};
pub use opts::{LogFormat, LogLevel, LogOpts, OnOff, StatsOpts, TriState};

#[cfg(feature = "props")]
pub mod props;
