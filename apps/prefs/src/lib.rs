// SPDX-License-Identifier: MIT OR Apache-2.0
//! prefs: MixOS's preferences, a nod to the Prefs drawer of AmigaOS. One
//! window, one sidebar of editors; the first is Applications, which shows
//! the apps the `releases` service follows and installs, updates, rolls
//! back and removes them.
//!
//! The behaviour is the `preferences` engine; this crate is the egui shell
//! around it (AGENTS.md §4.1): [`view`] draws the engine's state, [`shell`]
//! runs its effects, and [`commands`] is the registry every action goes
//! through, from the menu, a button, a shortcut or the Bus.
pub mod commands;
pub mod shell;
pub mod view;

use toolkit::Strings;

/// The English catalogue.
pub const CATALOGUE: &str = include_str!("../i18n/en/prefs.ftl");

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

/// `key` from the catalogue with named arguments: the engine's labeller.
pub fn label_with(key: &str, args: &[(&str, &str)]) -> String {
    STRINGS.with(|s| s.with(key, args))
}
