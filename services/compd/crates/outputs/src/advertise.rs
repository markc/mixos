// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
// Developer logging: bring error!/warn!/info!/trace!/abort! into scope for every module in
// this crate. (Drop this line if the crate genuinely never logs.)

// Retained legacy path: docs/docs/decisions/2026-10-11-compd-module-layout.md.
#[expect(clippy::module_inception)]
pub mod advertise;
