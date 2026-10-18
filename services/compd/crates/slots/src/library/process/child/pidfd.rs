// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! A file descriptor per child, so a child's exit is an ordinary event on the loop
//! rather than a signal plus an indiscriminate wait.

// Developer logging: error!/warn!/info!/trace!/abort! in scope for this crate.

pub mod pidfd;
