// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The desktop's appearance context. The 0.8.0 portal projection that lived
//! here is gone: portald reads the `values` view and `settings.palette`.

/// The effective context whose look the desktop draws: its resolved accent
/// is the one topics and `settings.palette` report.
pub const APPEARANCE_CONTEXT: &str = "desktop";
