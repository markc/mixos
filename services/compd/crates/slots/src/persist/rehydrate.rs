// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
// Developer logging: bring error!/warn!/info!/trace!/abort! into scope.

// World-start rehydration: load each persisted slot from disk and write the
// reconstructed live value back. Missing files are a normal first run; corrupt
// files are quarantined and defaults kept — never fatal.
pub mod base;
