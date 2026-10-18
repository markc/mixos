// SPDX-License-Identifier: MIT OR Apache-2.0
//! Refcount reconciler + blob sweep.
//! Phase 3 lands the ATTACH-DATABASE pattern, the async reconciler task,
//! and the dry-run / live `gc()` entry point.
