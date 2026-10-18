// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Child hygiene: what a spawned process must NOT inherit from the compositor —
//! the reaper's blocked signals, our file descriptors, our CPU-priority boost.
//! Runs in the forked child before `exec`; EVERY `Command` the compositor
//! spawns installs it as the `pre_exec` hook, app launches and short-lived
//! helpers alike.

// Developer logging: error!/warn!/info!/trace!/abort! in scope for this crate.

pub mod hygiene;
