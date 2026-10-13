// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! dopus's default key bindings. The person's own bindings live in the
//! desktop profile (`settings::keymap`, under `dopus`); the shell applies
//! them over these with `toolkit::Registry::rebind`. Chord text follows
//! [`toolkit::keymap`].

use egui::KeyboardShortcut;

/// The default chords, `(chord, action)`, in the order menus show them: an
/// action's first chord is its menu accelerator.
pub const DEFAULT: &[(&str, &str)] = &[
    ("Ctrl+B", "view.toggle-places"),
    ("Ctrl+I", "view.toggle-properties"),
    ("Ctrl+L", "location.focus"),
    ("Enter", "file.open"),
    ("Ctrl+Shift+N", "file.new-folder"),
    ("F2", "file.rename"),
    ("Ctrl+E", "file.rename"),
    ("Ctrl+C", "file.copy-other-pane"),
    ("Ctrl+X", "file.move-other-pane"),
    ("Delete", "file.delete"),
    ("Ctrl+Q", "app.quit"),
    ("Alt+ArrowLeft", "nav.back"),
    ("Alt+ArrowRight", "nav.forward"),
    ("Backspace", "nav.parent"),
    ("Alt+Home", "nav.home"),
    ("F6", "nav.switch-pane"),
    ("Tab", "nav.switch-pane"),
    ("F5", "view.refresh"),
    ("Ctrl+R", "view.refresh"),
    ("Ctrl+H", "view.toggle-hidden"),
    ("Ctrl+1", "view.sort-name"),
    ("Ctrl+2", "view.sort-size"),
    ("Ctrl+3", "view.sort-modified"),
    ("ArrowDown", "selection.next"),
    ("ArrowUp", "selection.previous"),
    ("Home", "selection.first"),
    ("End", "selection.last"),
    ("Ctrl+Alt+D", "theme.mode-toggle"),
    ("Ctrl+Alt+1", "theme.scheme-ocean"),
    ("Ctrl+Alt+2", "theme.scheme-crimson"),
    ("Ctrl+Alt+3", "theme.scheme-stone"),
    ("Ctrl+Alt+4", "theme.scheme-forest"),
    ("Ctrl+Alt+5", "theme.scheme-sunset"),
    ("Ctrl+Alt+6", "theme.scheme-mono"),
];

/// `action`'s default chords, in order.
pub fn defaults_for(action: &str) -> Vec<KeyboardShortcut> {
    DEFAULT
        .iter()
        .filter(|(_, a)| *a == action)
        .map(|(c, _)| toolkit::keymap::parse_chord(c).expect("a default chord parses"))
        .collect()
}

/// `action`'s default chords as text.
pub fn default_keys(action: &str) -> Vec<String> {
    defaults_for(action)
        .iter()
        .map(toolkit::keymap::chord_text)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_default_parses_once_and_round_trips() {
        let mut seen = std::collections::HashSet::new();
        for (text, action) in DEFAULT {
            let s = toolkit::keymap::parse_chord(text)
                .unwrap_or_else(|| panic!("{text} does not parse"));
            assert!(seen.insert(s), "{text} bound twice (again to {action})");
            assert_eq!(&toolkit::keymap::chord_text(&s), text, "round trip");
        }
        assert_eq!(default_keys("file.rename"), ["F2", "Ctrl+E"]);
        assert!(defaults_for("help.about").is_empty());
    }
}
