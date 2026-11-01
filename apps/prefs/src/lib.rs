// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! prefs: MixOS's Preferences, in COSMIC Settings' shape: a sidebar of
//! the eleven top-level pages, sub-pages with a back link, and titled list
//! cards of setting rows that apply at once through settingsd.
//!
//! The behaviour is the `preferences` engine (its page data, store and
//! navigation); this crate is the egui shell around it (AGENTS.md §4.1):
//! [`cosmic`] draws the window, [`view`] the editors that keep their own
//! layout, [`shell`] runs the effects, and [`actions`] is the registry every
//! action goes through, from a click, a shortcut or the Bus.
pub mod actions;
pub mod cosmic;
mod font_families;
mod scan;
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

#[cfg(test)]
mod tests {
    #[test]
    fn every_appearance_style_has_a_localised_label() {
        for style in design::Style::ALL {
            let key = format!("look-style-{}", style.name());
            assert_ne!(
                super::label(&key),
                key,
                "{style:?} must not display its message id"
            );
        }
    }
}
