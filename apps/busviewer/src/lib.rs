// SPDX-License-Identifier: MIT OR Apache-2.0
//! busviewer: browse the native ABP services on this node, read their verb
//! descriptions and mesh membership, and call a verb once with a JSON body.
//!
//! The behaviour is the `inspector` engine; this crate is the egui shell
//! around it (AGENTS.md §4.1): [`view`] draws the engine's state, [`shell`]
//! runs its effects, and [`commands`] is the registry every action goes
//! through, from the menu, a shortcut or the Bus.
pub mod commands;
pub mod shell;
pub mod view;

use toolkit::Strings;

/// The English catalogue.
pub const CATALOGUE: &str = include_str!("../i18n/en/busviewer.ftl");

/// The loaded catalogue.
pub fn strings() -> Strings {
    Strings::new(CATALOGUE)
}

thread_local! {
    static STRINGS: Strings = strings();
}

/// The engine's labeller: `key` from the catalogue.
pub fn label(key: &str) -> String {
    STRINGS.with(|s| s.get(key))
}
