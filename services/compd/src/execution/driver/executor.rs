// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The app-launch `Executor` — kernel driver data. Spawns apps (inline or via
//! the off-thread worker), injects the faithful base environment, and posts
//! every outcome onto a calloop channel for orchestration to broadcast.

pub mod executor;
pub mod install;
