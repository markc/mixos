// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
// Developer logging: bring error!/warn!/info!/trace!/abort! into scope.

// The atomic file write (temp + fsync + rename) and the long-lived worker thread
// that runs it off the frame thread, so a blocking fsync never stalls a frame.
pub mod base;
