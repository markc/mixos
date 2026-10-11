// SPDX-License-Identifier: MIT OR Apache-2.0
//! ced, the MixOS Editor: a desktop editor over the `edit` Bus service.
//! Several people and agents can edit the same document at once; ced shows
//! their carets and changes as they happen.
//!
//! The behaviour is the `documents` engine (open documents, actions, the
//! `ced.*` verbs, sessions); the text view is the shared `editor` library.
//! This crate is the shell around them (AGENTS.md §4.1):
//!
//! - [`bus`]: the Bus thread (calls to the edit service, `ced.*` commands,
//!   topics, timers);
//! - [`headless`]: `ced --headless`, every verb with no window;
//! - [`app`]: the window's state around the controller;
//! - [`commands`]: the registry every action goes through;
//! - [`view`]: the window's drawing; [`shell`]: the running window.

pub mod app;
pub mod bus;
pub mod commands;
pub mod headless;
pub mod shell;
pub mod view;

use toolkit::Strings;

/// The English catalogue.
pub const CATALOGUE: &str = include_str!("../i18n/en/ced.ftl");

/// The loaded catalogue.
pub fn strings() -> Strings {
    Strings::new(CATALOGUE)
}

thread_local! {
    static STRINGS: Strings = strings();
}

/// `key` from the catalogue.
pub fn label(key: &str) -> String {
    STRINGS.with(|s| s.get(key))
}

/// `key` from the catalogue with named arguments.
pub fn label_with(key: &str, args: &[(&str, &str)]) -> String {
    STRINGS.with(|s| s.with(key, args))
}

/// A run id for the op ids this process sends (they must differ between
/// runs so the edit service never mistakes a new op for a replay).
pub fn run_id() -> u32 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    nanos ^ std::process::id().rotate_left(16)
}
