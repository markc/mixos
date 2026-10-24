// SPDX-License-Identifier: MIT OR Apache-2.0
//! Shared Mix macro discovery and execution. Discovery never executes scripts.

mod catalogue;
mod runner;
pub mod session;
pub mod verbs;

pub use catalogue::{
    Catalogue, Diagnostic, Entry, Library, Roots, Targets, Tier, check_editable, valid_id,
};
pub use runner::{OUTPUT_LIMIT, Output, Runner, Status};
pub use session::Session;
