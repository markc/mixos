// SPDX-License-Identifier: MIT OR Apache-2.0
//! Account-owned Sieve. Compilation and evaluation have no I/O; delivery applies
//! a fully validated action plan only after evaluation succeeds.

pub mod delivery;
mod interpreter;
mod mailboxes;
pub mod server;
pub mod store;

pub use interpreter::{Context, Plan, Script, Vacation, compile};

pub const EXTENSIONS: &str = "fileinto mailbox envelope imap4flags copy body variables vacation relational comparator-i;ascii-numeric comparator-i;ascii-casemap comparator-i;octet reject ereject index";
pub const MAX_SCRIPT_BYTES: usize = 64 * 1024;
pub const MAX_SCRIPTS: usize = 32;
pub const MAX_STEPS: usize = 10_000;
pub const MAX_REDIRECTS: usize = 4;
pub const MAX_ACTIONS: usize = 64;
